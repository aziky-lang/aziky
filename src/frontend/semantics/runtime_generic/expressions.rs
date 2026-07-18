//! Runtime-native expression lowering and scalar type inference.

use super::*;

impl<'a> RuntimeGenericBuilder<'a> {
    pub(super) fn lower_cond(&mut self, expr: &Expr) -> RuntimeGenericLowerResult<usize> {
        if let Expr::Binary {
            op, left, right, ..
        } = expr
        {
            if let Some(result) = self.try_lower_float_comparison(*op, left, right)? {
                return Ok(result);
            }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
            ) {
                let (cmp, lhs, rhs) = self.lower_cmp_operands(*op, left, right)?;
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::Cmp {
                    dst,
                    op: cmp,
                    lhs,
                    rhs,
                });
                return Ok(dst);
            }
        }

        let cond_ty = self.infer_expr_int_type(expr)?;
        let value = self.lower_expr_as_type(expr, cond_ty)?;
        let dst = self.alloc_slot();
        self.emit(RuntimeInstr::Cmp {
            dst,
            op: RuntimeCmpOp::Ne,
            lhs: value,
            rhs: RuntimeOperand::Imm(0),
        });
        Ok(dst)
    }

    pub(super) fn emit_jump_if_false(&mut self, expr: &Expr) -> RuntimeGenericLowerResult<usize> {
        if let Expr::Binary {
            op, left, right, ..
        } = expr
        {
            if let Some(result) = self.try_lower_float_comparison(*op, left, right)? {
                return Ok(self.emit(RuntimeInstr::JumpIfZero {
                    cond_slot: result,
                    target: usize::MAX,
                }));
            }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
            ) {
                let (cmp, lhs, rhs) = self.lower_cmp_operands(*op, left, right)?;
                let idx = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: cmp,
                    lhs,
                    rhs,
                    target: usize::MAX,
                });
                return Ok(idx);
            }
        }

        let cond_slot = self.lower_cond(expr)?;
        Ok(self.emit(RuntimeInstr::JumpIfZero {
            cond_slot,
            target: usize::MAX,
        }))
    }

    pub(super) fn lower_expr_as_scalar(
        &mut self,
        expr: &Expr,
        scalar_ty: RuntimeScalarType,
    ) -> RuntimeGenericLowerResult<RuntimeOperand> {
        match scalar_ty {
            RuntimeScalarType::Int(int_ty) => self.lower_expr_as_type(expr, int_ty),
            RuntimeScalarType::Float(bits) => self.lower_expr_as_float(expr, bits),
        }
    }

    pub(super) fn lower_expr_as_float(
        &mut self,
        expr: &Expr,
        bits: u16,
    ) -> RuntimeGenericLowerResult<RuntimeOperand> {
        match expr {
            Expr::Match { value, arms, span } => {
                self.lower_runtime_match_scalar(value, arms, RuntimeScalarType::Float(bits), *span)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if self
                .runtime_user_method_scalar_type(receiver, name, args, *span)?
                .is_some() =>
            {
                let return_layout = self
                    .lower_runtime_user_method_call(receiver, name, args, *span)?
                    .flatten();
                let Some(RuntimeFunctionReturnLayout::Scalar {
                    ty: RuntimeScalarType::Float(return_bits),
                    slot,
                }) = return_layout
                else {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime method return type is not floating-point",
                        *span,
                    )));
                };
                if return_bits != bits {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime method floating-point return width mismatch",
                        *span,
                    )));
                }
                Ok(RuntimeOperand::Slot(slot))
            }
            Expr::FieldAccess { base, field, span } => {
                if let Some((operand, field_ty)) =
                    self.lower_struct_slot_field_access(base, field, *span)?
                {
                    if field_ty != RuntimeScalarType::Float(bits) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "runtime generic type mismatch: expected f{bits}, found {}",
                                field_ty.display()
                            )
                            .as_str(),
                            *span,
                        )));
                    }
                    return Ok(operand);
                }
                Err(RuntimeGenericLowerError::Unsupported)
            }
            Expr::Index { base, index, span } => {
                let native_index = if let Expr::Ident { name, .. } = base.as_ref() {
                    self.lower_owned_list_scalar_checked_index(name, index, *span)?
                } else {
                    self.lower_owned_struct_list_scalar_checked_index(base, index, *span)?
                };
                let Some((ptr_slot, index_operand, elem_ty)) = native_index else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if elem_ty != RuntimeScalarType::Float(bits) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "runtime generic type mismatch: expected f{bits}, found {}",
                            elem_ty.display()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::HeapLoadInt {
                    dst,
                    ptr: RuntimeOperand::Slot(ptr_slot),
                    index: index_operand,
                    bytes: elem_ty.storage_bytes(),
                });
                Ok(RuntimeOperand::Slot(dst))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "unwrap_or" && self.is_native_option_scalar_expr(receiver) => {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "unwrap_or() expects exactly one fallback argument",
                        *span,
                    )));
                }
                let elem_ty = self
                    .native_option_scalar_type(receiver)
                    .ok_or(RuntimeGenericLowerError::Unsupported)?;
                if elem_ty != RuntimeScalarType::Float(bits) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime Option unwrap_or() payload type mismatch",
                        *span,
                    )));
                }
                let fallback = self.lower_expr_as_float(&args[0], bits)?;
                let (tag, payload, _) =
                    self.lower_expr_as_option_scalar(receiver, Some(elem_ty))?;
                let result_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: result_slot,
                    src: fallback,
                });
                let absent = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::Ne,
                    lhs: tag,
                    rhs: RuntimeOperand::Imm(0),
                    target: usize::MAX,
                });
                self.emit(RuntimeInstr::Mov {
                    dst: result_slot,
                    src: payload,
                });
                let done = self.instrs.len();
                self.patch_target(absent, done)?;
                Ok(RuntimeOperand::Slot(result_slot))
            }
            Expr::Number { literal, span } => {
                let parsed = parse_number_literal(literal, *span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                let value = match parsed {
                    Value::Float { value, .. } => value,
                    Value::Int { value, .. } => value as f64,
                    Value::UInt { value, .. } => value as f64,
                    _ => return Err(RuntimeGenericLowerError::Unsupported),
                };
                Ok(RuntimeOperand::Imm(encode_float_bits(value, bits, *span)?))
            }
            Expr::Ident { name, span } => {
                let binding = self.scopes.get(name).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("unknown identifier '{name}'"),
                        span,
                    ))
                })?;
                if matches!(binding, RuntimeGenericBinding::OwnedPtr { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "owned heap pointers cannot be copied, converted, or used as scalar values",
                        *span,
                    )));
                }
                if matches!(binding, RuntimeGenericBinding::OwnedFile { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "owned file handles cannot be copied, converted, or used as scalar values",
                        *span,
                    )));
                }
                let Some((slot, _, binding_ty)) = binding.as_scalar() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if binding_ty != RuntimeScalarType::Float(bits) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "runtime generic type mismatch: expected f{}, found {}",
                            bits,
                            binding_ty.display()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                Ok(RuntimeOperand::Slot(slot))
            }
            Expr::Call { name, span, args } => {
                if name == "runtime_seed" {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime_seed() cannot be lowered as float",
                        *span,
                    )));
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
                let Some(RuntimeFunctionReturnLayout::Scalar {
                    ty: ret_ty,
                    slot: ret_slot,
                }) = self.function_return_layout(name)?
                else {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "expression call requires function with return type",
                        *span,
                    )));
                };
                if ret_ty != RuntimeScalarType::Float(bits) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "runtime generic type mismatch: expected f{}, found {}",
                            bits,
                            ret_ty.display()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                self.emit_call_target(name, *span);
                Ok(RuntimeOperand::Slot(ret_slot))
            }
            Expr::Unary {
                op: UnaryOp::Plus,
                expr,
                ..
            } => self.lower_expr_as_float(expr, bits),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                span,
            } => {
                let rhs = self.lower_expr_as_float(expr, bits)?;
                let zero = RuntimeOperand::Imm(encode_float_bits(0.0, bits, *span)?);
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::FloatBinOp {
                    dst,
                    bits,
                    op: RuntimeFloatBinOp::Sub,
                    lhs: zero,
                    rhs,
                });
                Ok(RuntimeOperand::Slot(dst))
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let float_op = match op {
                    BinaryOp::Add => RuntimeFloatBinOp::Add,
                    BinaryOp::Sub => RuntimeFloatBinOp::Sub,
                    BinaryOp::Mul => RuntimeFloatBinOp::Mul,
                    BinaryOp::Div => RuntimeFloatBinOp::Div,
                    _ => return Err(RuntimeGenericLowerError::Unsupported),
                };
                let lhs = self.lower_expr_as_float(left, bits)?;
                let rhs = self.lower_expr_as_float(right, bits)?;
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::FloatBinOp {
                    dst,
                    bits,
                    op: float_op,
                    lhs,
                    rhs,
                });
                Ok(RuntimeOperand::Slot(dst))
            }
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn lower_expr_as_type(
        &mut self,
        expr: &Expr,
        int_ty: RuntimeIntType,
    ) -> RuntimeGenericLowerResult<RuntimeOperand> {
        match expr {
            Expr::Match { value, arms, span } => {
                self.lower_runtime_match_scalar(value, arms, RuntimeScalarType::Int(int_ty), *span)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "char_count" => {
                if !args.is_empty() || int_ty != RuntimeIntType::new(false, 64)? {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "char_count() expects no arguments and requires u64 context",
                        *span,
                    )));
                }
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let Some((ptr_slot, len_slot, _, _, _)) = self
                    .scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_string)
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                self.emit_owned_string_char_count(ptr_slot, len_slot)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if self
                .runtime_user_method_scalar_type(receiver, name, args, *span)?
                .is_some() =>
            {
                let return_layout = self
                    .lower_runtime_user_method_call(receiver, name, args, *span)?
                    .flatten();
                let Some(RuntimeFunctionReturnLayout::Scalar {
                    ty: RuntimeScalarType::Int(return_ty),
                    slot,
                }) = return_layout
                else {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime method return type is not an integer",
                        *span,
                    )));
                };
                if return_ty != int_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime method integer return type mismatch",
                        *span,
                    )));
                }
                Ok(RuntimeOperand::Slot(slot))
            }
            Expr::Bool { value, span } => {
                let encoded = int_ty.encode_value(Value::Bool(*value), *span)?;
                Ok(RuntimeOperand::Imm(encoded))
            }
            Expr::Char { value, span } => {
                let encoded = int_ty.encode_value(Value::Char(*value), *span)?;
                Ok(RuntimeOperand::Imm(encoded))
            }
            Expr::Number { literal, span } => {
                let value = parse_number_literal(literal, *span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                let encoded = int_ty.encode_value(value, *span)?;
                Ok(RuntimeOperand::Imm(encoded))
            }
            Expr::Ident { name, span } => {
                let binding = self.scopes.get(name).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("unknown identifier '{name}'"),
                        span,
                    ))
                })?;
                if matches!(binding, RuntimeGenericBinding::OwnedPtr { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "owned heap pointers cannot be copied, converted, or used as scalar values",
                        *span,
                    )));
                }
                if matches!(binding, RuntimeGenericBinding::OwnedFile { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "owned file handles cannot be copied, converted, or used as scalar values",
                        *span,
                    )));
                }
                let Some((slot, _, binding_ty)) = binding.as_scalar() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if binding_ty != RuntimeScalarType::Int(int_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "runtime generic type mismatch: expected {}, found {}",
                            int_ty.display(),
                            binding_ty.display()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                Ok(RuntimeOperand::Slot(slot))
            }
            Expr::FieldAccess { base, field, span } => {
                if let Some((operand, value_ty)) =
                    self.lower_struct_slot_field_access(base, field, *span)?
                {
                    if value_ty != RuntimeScalarType::Int(int_ty) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "runtime generic type mismatch: expected {}, found {}",
                                int_ty.display(),
                                value_ty.display()
                            )
                            .as_str(),
                            *span,
                        )));
                    }
                    return Ok(operand);
                }
                let value = self.lower_const_struct_field(base, field, *span)?;
                if value.ty != int_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "runtime generic type mismatch: expected {}, found {}",
                            int_ty.display(),
                            value.ty.display()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                Ok(RuntimeOperand::Imm(value.encoded))
            }
            Expr::Index { base, index, span } => {
                if let Expr::Ident { name, .. } = base.as_ref() {
                    if let Some((ptr_slot, index_operand, elem_ty)) =
                        self.lower_owned_list_scalar_checked_index(name, index, *span)?
                    {
                        if RuntimeScalarType::Int(int_ty) != elem_ty {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "runtime generic type mismatch: expected {}, found {}",
                                    int_ty.display(),
                                    elem_ty.display()
                                )
                                .as_str(),
                                *span,
                            )));
                        }
                        let dst = self.alloc_slot();
                        self.emit(RuntimeInstr::HeapLoadInt {
                            dst,
                            ptr: RuntimeOperand::Slot(ptr_slot),
                            index: index_operand,
                            bytes: elem_ty.storage_bytes(),
                        });
                        self.normalize_scalar_slot(dst, elem_ty);
                        return Ok(RuntimeOperand::Slot(dst));
                    }
                }
                if let Some((ptr_slot, index_operand, elem_ty)) =
                    self.lower_owned_struct_list_scalar_checked_index(base, index, *span)?
                {
                    if RuntimeScalarType::Int(int_ty) != elem_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "runtime generic type mismatch: expected {}, found {}",
                                int_ty.display(),
                                elem_ty.display()
                            )
                            .as_str(),
                            *span,
                        )));
                    }
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst,
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        index: index_operand,
                        bytes: elem_ty.storage_bytes(),
                    });
                    self.normalize_scalar_slot(dst, elem_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }
                if let Some((operand, value_ty)) =
                    self.lower_array_slot_index_access(base, index, *span)?
                {
                    if value_ty != int_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "runtime generic type mismatch: expected {}, found {}",
                                int_ty.display(),
                                value_ty.display()
                            )
                            .as_str(),
                            *span,
                        )));
                    }
                    return Ok(operand);
                }
                if let Some((operand, value_ty)) =
                    self.lower_dict_slot_index_access(base, index, *span)?
                {
                    if value_ty != int_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "runtime generic type mismatch: expected {}, found {}",
                                int_ty.display(),
                                value_ty.display()
                            )
                            .as_str(),
                            *span,
                        )));
                    }
                    return Ok(operand);
                }
                match self.lower_const_index_access(base, index, *span) {
                    Ok(value) => {
                        if value.ty != int_ty {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "runtime generic type mismatch: expected {}, found {}",
                                    int_ty.display(),
                                    value.ty.display()
                                )
                                .as_str(),
                                *span,
                            )));
                        }
                        Ok(RuntimeOperand::Imm(value.encoded))
                    }
                    Err(RuntimeGenericLowerError::Unsupported) => {
                        let Some((operand, value_ty)) =
                            self.lower_const_array_dynamic_index(base, index, *span)?
                        else {
                            return Err(RuntimeGenericLowerError::Unsupported);
                        };
                        if value_ty != int_ty {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "runtime generic type mismatch: expected {}, found {}",
                                    int_ty.display(),
                                    value_ty.display()
                                )
                                .as_str(),
                                *span,
                            )));
                        }
                        Ok(operand)
                    }
                    Err(err) => Err(err),
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_ok" | "is_err" | "unwrap_or")
                && self.native_result_ok_scalar_type(receiver).is_some() =>
            {
                let (tag_slot, payload_slot, ok_tag, ok_ty) = self
                    .native_result_ok_scalar_parts(receiver)
                    .ok_or(RuntimeGenericLowerError::Unsupported)?;
                if name == "unwrap_or" {
                    if args.len() != 1 || ok_ty != RuntimeScalarType::Int(int_ty) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Result.unwrap_or() fallback must match its Ok integer type",
                            *span,
                        )));
                    }
                    let fallback = self.lower_expr_as_scalar(&args[0], ok_ty)?;
                    let result = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: result,
                        src: fallback,
                    });
                    let not_ok = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(tag_slot),
                        rhs: RuntimeOperand::Imm(ok_tag),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst: result,
                        src: RuntimeOperand::Slot(payload_slot),
                    });
                    self.patch_target(not_ok, self.instrs.len())?;
                    self.normalize_scalar_slot(result, ok_ty);
                    return Ok(RuntimeOperand::Slot(result));
                }
                if !args.is_empty() || int_ty != RuntimeIntType::new(false, 8)? {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("Result.{name}() takes no arguments and returns bool").as_str(),
                        *span,
                    )));
                }
                let result = self.alloc_slot();
                self.emit(RuntimeInstr::Cmp {
                    dst: result,
                    op: if name == "is_ok" {
                        RuntimeCmpOp::Eq
                    } else {
                        RuntimeCmpOp::Ne
                    },
                    lhs: RuntimeOperand::Slot(tag_slot),
                    rhs: RuntimeOperand::Imm(ok_tag),
                });
                Ok(RuntimeOperand::Slot(result))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_some" | "is_none")
                && self.native_option_struct_layout(receiver).is_some() =>
            {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    )));
                }
                let bool_ty = RuntimeIntType::new(false, 8)?;
                if int_ty != bool_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("runtime Option.{name}() requires bool context").as_str(),
                        *span,
                    )));
                }
                let layout = self
                    .native_option_struct_layout(receiver)
                    .ok_or(RuntimeGenericLowerError::Unsupported)?;
                let (tag, _, _) = self.lower_expr_as_option_struct(receiver, Some(&layout))?;
                let result_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Cmp {
                    dst: result_slot,
                    op: if name == "is_some" {
                        RuntimeCmpOp::Ne
                    } else {
                        RuntimeCmpOp::Eq
                    },
                    lhs: tag,
                    rhs: RuntimeOperand::Imm(0),
                });
                Ok(RuntimeOperand::Slot(result_slot))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_some" | "is_none" | "unwrap_or")
                && self.is_native_option_scalar_expr(receiver) =>
            {
                if name == "unwrap_or" {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "unwrap_or() expects exactly one fallback argument",
                            *span,
                        )));
                    }
                    let elem_ty = self
                        .native_option_scalar_type(receiver)
                        .ok_or(RuntimeGenericLowerError::Unsupported)?;
                    if RuntimeScalarType::Int(int_ty) != elem_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime Option unwrap_or() payload type mismatch",
                            *span,
                        )));
                    }
                    let fallback = self.lower_expr_as_scalar(&args[0], elem_ty)?;
                    let (tag, payload, _) =
                        self.lower_expr_as_option_scalar(receiver, Some(elem_ty))?;
                    let result_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: result_slot,
                        src: fallback,
                    });
                    let absent = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Ne,
                        lhs: tag,
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst: result_slot,
                        src: payload,
                    });
                    let done = self.instrs.len();
                    self.patch_target(absent, done)?;
                    self.normalize_scalar_slot(result_slot, elem_ty);
                    return Ok(RuntimeOperand::Slot(result_slot));
                }

                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    )));
                }
                let bool_ty = RuntimeIntType::new(false, 8)?;
                if int_ty != bool_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("runtime Option.{name}() requires bool context").as_str(),
                        *span,
                    )));
                }
                let elem_ty = self
                    .native_option_scalar_type(receiver)
                    .ok_or(RuntimeGenericLowerError::Unsupported)?;
                let (tag, _, _) = self.lower_expr_as_option_scalar(receiver, Some(elem_ty))?;
                let result_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Cmp {
                    dst: result_slot,
                    op: if name == "is_some" {
                        RuntimeCmpOp::Ne
                    } else {
                        RuntimeCmpOp::Eq
                    },
                    lhs: tag,
                    rhs: RuntimeOperand::Imm(0),
                });
                Ok(RuntimeOperand::Slot(result_slot))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "contains" => {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime contains() expects exactly one argument",
                        *span,
                    )));
                }
                let list = if let Expr::Ident { name, .. } = receiver.as_ref() {
                    self.scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                        .map(|(ptr, len, _, _, _, elem)| (ptr, len, elem))
                } else {
                    match self.owned_struct_list_field(receiver) {
                        Some(RuntimeOwnedStructListField::Scalar {
                            ptr_slot,
                            len_slot,
                            elem_ty,
                            ..
                        }) => Some((ptr_slot, len_slot, elem_ty)),
                        _ => None,
                    }
                };
                let Some((ptr_slot, len_slot, elem_ty)) = list else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let bool_ty = RuntimeIntType::new(false, 8)?;
                if int_ty != bool_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime list contains() requires bool context",
                        *span,
                    )));
                }
                let needle = self.lower_expr_as_scalar(&args[0], elem_ty)?;
                let needle = self.canonicalize_scalar_operand(needle, elem_ty);
                let index_slot = self.alloc_slot();
                let elem_slot = self.alloc_slot();
                let result_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: index_slot,
                    src: RuntimeOperand::Imm(0),
                });
                self.emit(RuntimeInstr::Mov {
                    dst: result_slot,
                    src: RuntimeOperand::Imm(0),
                });
                let loop_start = self.instrs.len();
                let exhausted = self.emit(RuntimeInstr::JumpIfCmpFalse {
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
                let not_found_here = match elem_ty {
                    RuntimeScalarType::Int(_) => self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(elem_slot),
                        rhs: needle,
                        target: usize::MAX,
                    }),
                    RuntimeScalarType::Float(bits) => {
                        let equal_slot =
                            self.emit_float_eq(RuntimeOperand::Slot(elem_slot), needle, bits)?;
                        self.emit(RuntimeInstr::JumpIfZero {
                            cond_slot: equal_slot,
                            target: usize::MAX,
                        })
                    }
                };
                self.emit(RuntimeInstr::Mov {
                    dst: result_slot,
                    src: RuntimeOperand::Imm(1),
                });
                let found = self.emit(RuntimeInstr::Jump { target: usize::MAX });
                let next = self.instrs.len();
                self.patch_target(not_found_here, next)?;
                self.emit(RuntimeInstr::BinOpInPlace {
                    dst: index_slot,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(1),
                });
                self.emit(RuntimeInstr::Jump { target: loop_start });
                let done = self.instrs.len();
                self.patch_target(exhausted, done)?;
                self.patch_target(found, done)?;
                Ok(RuntimeOperand::Slot(result_slot))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "len" || name == "is_empty" => {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("runtime {}() expects no arguments", name).as_str(),
                        *span,
                    )));
                }
                if let Expr::Ident {
                    name: receiver_name,
                    span: receiver_span,
                } = receiver.as_ref()
                {
                    self.reject_moved_resource(receiver_name, *receiver_span)?;
                    let owned_len_slot = self.scopes.get(receiver_name).and_then(|binding| {
                        binding
                            .as_owned_list_scalar()
                            .map(|(_, len_slot, _, _, _, _)| len_slot)
                            .or_else(|| {
                                binding
                                    .as_owned_list_struct()
                                    .map(|(_, len_slot, _, _, _, _)| len_slot)
                            })
                            .or_else(|| {
                                binding
                                    .as_owned_string()
                                    .map(|(_, len_slot, _, _, _)| len_slot)
                            })
                            .or_else(|| {
                                binding
                                    .as_owned_map()
                                    .map(|(_, len_slot, _, _, _, _)| len_slot)
                            })
                    });
                    if let Some(len_slot) = owned_len_slot {
                        if name == "is_empty" {
                            let dst = self.alloc_slot();
                            self.emit(RuntimeInstr::Cmp {
                                dst,
                                op: RuntimeCmpOp::Eq,
                                lhs: RuntimeOperand::Slot(len_slot),
                                rhs: RuntimeOperand::Imm(0),
                            });
                            return Ok(RuntimeOperand::Slot(dst));
                        }
                        if !int_ty.signed && int_ty.bits == 64 {
                            return Ok(RuntimeOperand::Slot(len_slot));
                        }
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime owned list len() requires u64 context",
                            *span,
                        )));
                    }
                    if let Some((slots, len_slot, _, _, full_len_known)) = self
                        .scopes
                        .get(receiver_name)
                        .and_then(RuntimeGenericBinding::as_array_slots)
                    {
                        if name == "is_empty" {
                            if full_len_known {
                                let encoded =
                                    int_ty.encode_value(Value::Bool(slots.is_empty()), *span)?;
                                return Ok(RuntimeOperand::Imm(encoded));
                            }
                            let dst = self.alloc_slot();
                            self.emit(RuntimeInstr::Cmp {
                                dst,
                                op: RuntimeCmpOp::Eq,
                                lhs: RuntimeOperand::Slot(len_slot),
                                rhs: RuntimeOperand::Imm(0),
                            });
                            return Ok(RuntimeOperand::Slot(dst));
                        }
                        if full_len_known {
                            let encoded = int_ty.encode_value(
                                Value::UInt {
                                    bits: 64,
                                    value: slots.len() as u128,
                                },
                                *span,
                            )?;
                            return Ok(RuntimeOperand::Imm(encoded));
                        }
                        if !int_ty.signed && int_ty.bits == 64 {
                            return Ok(RuntimeOperand::Slot(len_slot));
                        }
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime mutable container len() currently requires u64 context",
                            *span,
                        )));
                    }
                    if let Some((entries, _, _)) = self
                        .scopes
                        .get(receiver_name)
                        .and_then(RuntimeGenericBinding::as_dict_slots)
                    {
                        let encoded = int_ty.encode_value(
                            Value::UInt {
                                bits: 64,
                                value: entries.len() as u128,
                            },
                            *span,
                        )?;
                        return Ok(RuntimeOperand::Imm(encoded));
                    }
                }
                if let Some(field) = self.owned_struct_list_field(receiver) {
                    let len_slot = match field {
                        RuntimeOwnedStructListField::Scalar { len_slot, .. }
                        | RuntimeOwnedStructListField::Struct { len_slot, .. } => len_slot,
                    };
                    if name == "is_empty" {
                        let dst = self.alloc_slot();
                        self.emit(RuntimeInstr::Cmp {
                            dst,
                            op: RuntimeCmpOp::Eq,
                            lhs: RuntimeOperand::Slot(len_slot),
                            rhs: RuntimeOperand::Imm(0),
                        });
                        return Ok(RuntimeOperand::Slot(dst));
                    }
                    if !int_ty.signed && int_ty.bits == 64 {
                        return Ok(RuntimeOperand::Slot(len_slot));
                    }
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime owned list len() requires u64 context",
                        *span,
                    )));
                }
                let len = match self.resolve_const_container(receiver) {
                    Some(RuntimeConstContainer::Array { elems }) => elems.len() as u64,
                    Some(RuntimeConstContainer::Dict { entries }) => entries.len() as u64,
                    _ => return Err(RuntimeGenericLowerError::Unsupported),
                };
                let encoded = if name == "is_empty" {
                    int_ty.encode_value(Value::Bool(len == 0), *span)?
                } else {
                    int_ty.encode_value(
                        Value::UInt {
                            bits: 64,
                            value: len as u128,
                        },
                        *span,
                    )?
                };
                Ok(RuntimeOperand::Imm(encoded))
            }
            Expr::Call { name, span, args } => {
                if name == "heap_alloc" {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "heap_alloc() is a linear resource and may only initialize a named immutable binding",
                        *span,
                    )));
                }
                if name == "heap_free" {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "heap_free() does not return a value",
                        *span,
                    )));
                }
                if name == "runtime_stdlib_abi_version" {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_stdlib_abi_version() does not accept arguments",
                            *span,
                        )));
                    }
                    return Ok(RuntimeOperand::Imm(
                        crate::COMPILER_STDLIB_ABI_VERSION as u64,
                    ));
                }
                if name == "runtime_environment_count" {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_environment_count() does not accept arguments",
                            *span,
                        )));
                    }
                    let entry_ptr = self.emit_entry_stack_pointer();
                    let argc_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst: argc_slot,
                        ptr: RuntimeOperand::Slot(entry_ptr),
                        index: RuntimeOperand::Imm(0),
                        bytes: 8,
                    });
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
                    self.normalize_slot(count_slot, int_ty);
                    return Ok(RuntimeOperand::Slot(count_slot));
                }
                let platform_load = match name.as_str() {
                    "runtime_arg_count" => Some(RuntimeLoadKind::ArgumentCount),
                    "runtime_monotonic_nanos" => Some(RuntimeLoadKind::MonotonicNanos),
                    "runtime_wall_time_nanos" => Some(RuntimeLoadKind::WallTimeNanos),
                    "runtime_process_id" => Some(RuntimeLoadKind::ProcessId),
                    _ => None,
                };
                if let Some(kind) = platform_load {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "platform load functions do not accept arguments",
                            *span,
                        )));
                    }
                    let is_clock = matches!(
                        kind,
                        RuntimeLoadKind::MonotonicNanos | RuntimeLoadKind::WallTimeNanos
                    );
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::LoadSeed {
                        dst,
                        kind,
                        input: None,
                    });
                    if is_clock {
                        self.emit_guard_failure_exit(
                            RuntimeCmpOp::Ne,
                            RuntimeOperand::Slot(dst),
                            RuntimeOperand::Imm(u64::MAX),
                            108,
                        )?;
                    }
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }
                if name == "runtime_seed" {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_seed() does not accept arguments",
                            *span,
                        )));
                    }
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::LoadSeed {
                        dst,
                        kind: RuntimeLoadKind::EntropySeed,
                        input: None,
                    });
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }
                if name == "runtime_bloom_sbbf_maybe" {
                    if args.len() != 2 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_bloom_sbbf_maybe() expects exactly two arguments (filter, hash)",
                            *span,
                        )));
                    }
                    let filter_slots = self.resolve_runtime_kernel_u64_array_slots(
                        "runtime_bloom_sbbf_maybe",
                        &args[0],
                        true,
                        *span,
                    )?;
                    if filter_slots.len() < 4
                        || (filter_slots.len() & 3) != 0
                        || !filter_slots.len().is_power_of_two()
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_bloom_sbbf_maybe() requires fixed [u64; N] with N power-of-two and divisible by 4",
                            *span,
                        )));
                    }
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let hash = self.lower_expr_as_type(&args[1], u64_ty)?;
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::BloomSplitBlockCheck {
                        dst,
                        filter_slots,
                        hash,
                    });
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }
                if name == "runtime_hash_probe_grouped16" {
                    if args.len() != 3 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_hash_probe_grouped16() expects exactly three arguments (ctrl, group_start, fingerprint)",
                            *span,
                        )));
                    }
                    let ctrl_slots = self.resolve_runtime_kernel_u64_array_slots(
                        "runtime_hash_probe_grouped16",
                        &args[0],
                        false,
                        *span,
                    )?;
                    if ctrl_slots.len() < 16 || !ctrl_slots.len().is_power_of_two() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_hash_probe_grouped16() requires fixed [u64; N] with N power-of-two and N >= 16",
                            *span,
                        )));
                    }
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let group_start = self.lower_expr_as_type(&args[1], u64_ty)?;
                    let fingerprint = self.lower_expr_as_type(&args[2], u64_ty)?;
                    let dst_mask = self.alloc_slot();
                    self.emit(RuntimeInstr::HashCtrlGroupProbe {
                        dst_mask,
                        ctrl_slots,
                        group_start,
                        fingerprint,
                    });
                    self.normalize_slot(dst_mask, int_ty);
                    return Ok(RuntimeOperand::Slot(dst_mask));
                }
                if name == "runtime_join_select_adaptive" {
                    if args.len() != 2 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime_join_select_adaptive() expects exactly two arguments (build_rows, probe_rows)",
                            *span,
                        )));
                    }
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let build_rows = self.lower_expr_as_type(&args[0], u64_ty)?;
                    let probe_rows = self.lower_expr_as_type(&args[1], u64_ty)?;
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::JoinSelectAdaptive {
                        dst,
                        build_rows,
                        probe_rows,
                    });
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
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
                let Some(RuntimeFunctionReturnLayout::Scalar {
                    ty: ret_ty,
                    slot: ret_slot,
                }) = self.function_return_layout(name)?
                else {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "expression call requires function with return type",
                        *span,
                    )));
                };
                if ret_ty != RuntimeScalarType::Int(int_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "runtime generic type mismatch: expected {}, found {}",
                            int_ty.display(),
                            ret_ty.display()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                self.emit_call_target(name, *span);
                Ok(RuntimeOperand::Slot(ret_slot))
            }
            Expr::Unary {
                op: UnaryOp::Plus,
                expr,
                ..
            } => self.lower_expr_as_type(expr, int_ty),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                span,
            } => {
                if !int_ty.signed {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "cannot negate unsigned integer in runtime generic lowering",
                        *span,
                    )));
                }
                let rhs = self.lower_expr_as_type(expr, int_ty)?;
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::BinOp {
                    dst,
                    op: RuntimeBinOp::Sub,
                    lhs: RuntimeOperand::Imm(0),
                    rhs,
                });
                self.normalize_slot(dst, int_ty);
                Ok(RuntimeOperand::Slot(dst))
            }
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
                span,
            } => {
                let bool_ty = RuntimeIntType::new(false, 8)?;
                if self.infer_expr_int_type(expr)? != bool_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "logical not expects bool operand in runtime generic lowering",
                        *span,
                    )));
                }
                let cond_slot = self.lower_cond(expr)?;
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::Cmp {
                    dst,
                    op: RuntimeCmpOp::Eq,
                    lhs: RuntimeOperand::Slot(cond_slot),
                    rhs: RuntimeOperand::Imm(0),
                });
                self.normalize_slot(dst, int_ty);
                Ok(RuntimeOperand::Slot(dst))
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                if matches!(
                    op,
                    BinaryOp::Eq
                        | BinaryOp::Ne
                        | BinaryOp::Lt
                        | BinaryOp::Le
                        | BinaryOp::Gt
                        | BinaryOp::Ge
                ) {
                    let bool_ty = RuntimeIntType::new(false, 8)?;
                    if int_ty != bool_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "comparison expressions require bool context in runtime generic lowering",
                            *span,
                        )));
                    }
                    let (cmp, lhs, rhs) = self.lower_cmp_operands(*op, left, right)?;
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::Cmp {
                        dst,
                        op: cmp,
                        lhs,
                        rhs,
                    });
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }

                if *op == BinaryOp::And {
                    let false_imm = int_ty.encode_value(Value::Bool(false), *span)?;
                    let true_imm = int_ty.encode_value(Value::Bool(true), *span)?;
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst,
                        src: RuntimeOperand::Imm(false_imm),
                    });
                    let left_cond = self.lower_cond(left)?;
                    let left_false = self.emit(RuntimeInstr::JumpIfZero {
                        cond_slot: left_cond,
                        target: usize::MAX,
                    });
                    let right_cond = self.lower_cond(right)?;
                    let right_false = self.emit(RuntimeInstr::JumpIfZero {
                        cond_slot: right_cond,
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst,
                        src: RuntimeOperand::Imm(true_imm),
                    });
                    let done = self.instrs.len();
                    self.patch_target(left_false, done)?;
                    self.patch_target(right_false, done)?;
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }

                if *op == BinaryOp::Or {
                    let false_imm = int_ty.encode_value(Value::Bool(false), *span)?;
                    let true_imm = int_ty.encode_value(Value::Bool(true), *span)?;
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst,
                        src: RuntimeOperand::Imm(true_imm),
                    });

                    let left_cond = self.lower_cond(left)?;
                    let left_true = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(left_cond),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    let right_cond = self.lower_cond(right)?;
                    let right_true = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(right_cond),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst,
                        src: RuntimeOperand::Imm(false_imm),
                    });
                    let done = self.instrs.len();
                    self.patch_target(left_true, done)?;
                    self.patch_target(right_true, done)?;
                    self.normalize_slot(dst, int_ty);
                    return Ok(RuntimeOperand::Slot(dst));
                }

                let bin = match op {
                    BinaryOp::Add => RuntimeBinOp::Add,
                    BinaryOp::Sub => RuntimeBinOp::Sub,
                    BinaryOp::Mul => RuntimeBinOp::Mul,
                    BinaryOp::Div => {
                        if int_ty.signed {
                            RuntimeBinOp::DivSigned
                        } else {
                            RuntimeBinOp::DivUnsigned
                        }
                    }
                    BinaryOp::Mod => {
                        if int_ty.signed {
                            RuntimeBinOp::ModSigned
                        } else {
                            RuntimeBinOp::ModUnsigned
                        }
                    }
                    BinaryOp::BitAnd => RuntimeBinOp::BitAnd,
                    BinaryOp::BitOr => RuntimeBinOp::BitOr,
                    BinaryOp::BitXor => RuntimeBinOp::BitXor,
                    BinaryOp::Shl => RuntimeBinOp::Shl,
                    BinaryOp::Shr => {
                        if int_ty.signed {
                            RuntimeBinOp::ShrSigned
                        } else {
                            RuntimeBinOp::ShrUnsigned
                        }
                    }
                    _ => return Err(RuntimeGenericLowerError::Unsupported),
                };
                let lhs = self.lower_expr_as_type(left, int_ty)?;
                let rhs = self.lower_expr_as_type(right, int_ty)?;
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::BinOp {
                    dst,
                    op: bin,
                    lhs,
                    rhs,
                });
                self.normalize_slot(dst, int_ty);
                Ok(RuntimeOperand::Slot(dst))
            }
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn normalize_slot(&mut self, slot: usize, int_ty: RuntimeIntType) {
        if int_ty.bits < 64 {
            self.emit(RuntimeInstr::NormalizeInt {
                dst: slot,
                signed: int_ty.signed,
                bits: int_ty.bits,
            });
        }
    }

    pub(super) fn canonicalize_int_operand(
        &mut self,
        operand: RuntimeOperand,
        int_ty: RuntimeIntType,
    ) -> RuntimeOperand {
        if !int_ty.signed || int_ty.bits == 64 {
            return operand;
        }
        let slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: slot,
            src: operand,
        });
        self.normalize_slot(slot, int_ty);
        RuntimeOperand::Slot(slot)
    }

    pub(super) fn canonicalize_scalar_operand(
        &mut self,
        operand: RuntimeOperand,
        scalar_ty: RuntimeScalarType,
    ) -> RuntimeOperand {
        match scalar_ty {
            RuntimeScalarType::Int(int_ty) => self.canonicalize_int_operand(operand, int_ty),
            RuntimeScalarType::Float(_) => operand,
        }
    }

    pub(super) fn emit_binop_slot(
        &mut self,
        op: RuntimeBinOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
    ) -> usize {
        let dst = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp { dst, op, lhs, rhs });
        dst
    }

    pub(super) fn emit_cmp_slot(
        &mut self,
        op: RuntimeCmpOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
    ) -> usize {
        let dst = self.alloc_slot();
        self.emit(RuntimeInstr::Cmp { dst, op, lhs, rhs });
        dst
    }

    pub(super) fn emit_float_eq(
        &mut self,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
        bits: u16,
    ) -> RuntimeGenericLowerResult<usize> {
        let (value_mask, abs_mask, exponent_mask, mantissa_mask) = match bits {
            32 => (
                u64::from(u32::MAX),
                u64::from(0x7fff_ffffu32),
                u64::from(0x7f80_0000u32),
                u64::from(0x007f_ffffu32),
            ),
            64 => (
                u64::MAX,
                0x7fff_ffff_ffff_ffff,
                0x7ff0_0000_0000_0000,
                0x000f_ffff_ffff_ffff,
            ),
            _ => return Err(RuntimeGenericLowerError::Unsupported),
        };

        let lhs_value =
            self.emit_binop_slot(RuntimeBinOp::BitAnd, lhs, RuntimeOperand::Imm(value_mask));
        let rhs_value =
            self.emit_binop_slot(RuntimeBinOp::BitAnd, rhs, RuntimeOperand::Imm(value_mask));
        let lhs_abs = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(lhs_value),
            RuntimeOperand::Imm(abs_mask),
        );
        let rhs_abs = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(rhs_value),
            RuntimeOperand::Imm(abs_mask),
        );

        let lhs_exp = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(lhs_abs),
            RuntimeOperand::Imm(exponent_mask),
        );
        let lhs_mantissa = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(lhs_abs),
            RuntimeOperand::Imm(mantissa_mask),
        );
        let lhs_exp_all = self.emit_cmp_slot(
            RuntimeCmpOp::Eq,
            RuntimeOperand::Slot(lhs_exp),
            RuntimeOperand::Imm(exponent_mask),
        );
        let lhs_mantissa_nonzero = self.emit_cmp_slot(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(lhs_mantissa),
            RuntimeOperand::Imm(0),
        );
        let lhs_nan = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(lhs_exp_all),
            RuntimeOperand::Slot(lhs_mantissa_nonzero),
        );

        let rhs_exp = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(rhs_abs),
            RuntimeOperand::Imm(exponent_mask),
        );
        let rhs_mantissa = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(rhs_abs),
            RuntimeOperand::Imm(mantissa_mask),
        );
        let rhs_exp_all = self.emit_cmp_slot(
            RuntimeCmpOp::Eq,
            RuntimeOperand::Slot(rhs_exp),
            RuntimeOperand::Imm(exponent_mask),
        );
        let rhs_mantissa_nonzero = self.emit_cmp_slot(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(rhs_mantissa),
            RuntimeOperand::Imm(0),
        );
        let rhs_nan = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(rhs_exp_all),
            RuntimeOperand::Slot(rhs_mantissa_nonzero),
        );
        let any_nan = self.emit_binop_slot(
            RuntimeBinOp::BitOr,
            RuntimeOperand::Slot(lhs_nan),
            RuntimeOperand::Slot(rhs_nan),
        );
        let ordered = self.emit_binop_slot(
            RuntimeBinOp::BitXor,
            RuntimeOperand::Slot(any_nan),
            RuntimeOperand::Imm(1),
        );

        let bits_equal = self.emit_cmp_slot(
            RuntimeCmpOp::Eq,
            RuntimeOperand::Slot(lhs_value),
            RuntimeOperand::Slot(rhs_value),
        );
        let lhs_zero = self.emit_cmp_slot(
            RuntimeCmpOp::Eq,
            RuntimeOperand::Slot(lhs_abs),
            RuntimeOperand::Imm(0),
        );
        let rhs_zero = self.emit_cmp_slot(
            RuntimeCmpOp::Eq,
            RuntimeOperand::Slot(rhs_abs),
            RuntimeOperand::Imm(0),
        );
        let both_zero = self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(lhs_zero),
            RuntimeOperand::Slot(rhs_zero),
        );
        let equal_or_zero = self.emit_binop_slot(
            RuntimeBinOp::BitOr,
            RuntimeOperand::Slot(bits_equal),
            RuntimeOperand::Slot(both_zero),
        );
        Ok(self.emit_binop_slot(
            RuntimeBinOp::BitAnd,
            RuntimeOperand::Slot(ordered),
            RuntimeOperand::Slot(equal_or_zero),
        ))
    }

    pub(super) fn normalize_scalar_slot(&mut self, slot: usize, scalar_ty: RuntimeScalarType) {
        if let RuntimeScalarType::Int(int_ty) = scalar_ty {
            self.normalize_slot(slot, int_ty);
        }
    }

    pub(super) fn infer_expr_scalar_type(
        &self,
        expr: &Expr,
    ) -> RuntimeGenericLowerResult<RuntimeScalarType> {
        match expr {
            Expr::Match { value, arms, span } => {
                self.infer_runtime_match_scalar_type(value, arms, *span)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "char_count" => {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "char_count() expects no arguments",
                        *span,
                    )));
                }
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if self
                    .scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_string)
                    .is_some()
                {
                    RuntimeIntType::new(false, 64).map(RuntimeScalarType::Int)
                } else {
                    Err(RuntimeGenericLowerError::Unsupported)
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if self
                .runtime_user_method_scalar_type(receiver, name, args, *span)?
                .is_some() =>
            {
                self.runtime_user_method_scalar_type(receiver, name, args, *span)?
                    .ok_or(RuntimeGenericLowerError::Unsupported)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_ok" | "is_err" | "unwrap_or")
                && self.native_result_ok_scalar_type(receiver).is_some() =>
            {
                match name.as_str() {
                    "is_ok" | "is_err" => {
                        if !args.is_empty() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("Result.{name}() expects no arguments").as_str(),
                                *span,
                            )));
                        }
                        Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 8)?))
                    }
                    "unwrap_or" => {
                        if args.len() != 1 {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "Result.unwrap_or() expects one fallback",
                                *span,
                            )));
                        }
                        self.native_result_ok_scalar_type(receiver)
                            .ok_or(RuntimeGenericLowerError::Unsupported)
                    }
                    _ => unreachable!("guarded Result method"),
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_some" | "is_none")
                && self.native_option_struct_layout(receiver).is_some() =>
            {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    )));
                }
                Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 8)?))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_some" | "is_none" | "unwrap_or")
                && self.is_native_option_scalar_expr(receiver) =>
            {
                match name.as_str() {
                    "is_some" | "is_none" => {
                        if !args.is_empty() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("{name}() expects no arguments").as_str(),
                                *span,
                            )));
                        }
                        Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 8)?))
                    }
                    "unwrap_or" => {
                        if args.len() != 1 {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "unwrap_or() expects exactly one fallback argument",
                                *span,
                            )));
                        }
                        self.native_option_scalar_type(receiver)
                            .ok_or(RuntimeGenericLowerError::Unsupported)
                    }
                    _ => unreachable!("guarded scalar Option method"),
                }
            }
            Expr::Index { base, index, span } => {
                if let Expr::Ident { name, .. } = base.as_ref() {
                    if let Some((_, _, _, _, _, elem_ty)) = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                    {
                        self.infer_expr_int_type(index).map_err(|err| match err {
                            RuntimeGenericLowerError::Unsupported => {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "list index must be integer in runtime generic lowering",
                                    *span,
                                ))
                            }
                            other => other,
                        })?;
                        return Ok(elem_ty);
                    }
                }
                if let Some(RuntimeOwnedStructListField::Scalar { elem_ty, .. }) =
                    self.owned_struct_list_field(base)
                {
                    self.infer_expr_int_type(index).map_err(|err| match err {
                        RuntimeGenericLowerError::Unsupported => {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "list index must be integer in runtime generic lowering",
                                *span,
                            ))
                        }
                        other => other,
                    })?;
                    return Ok(elem_ty);
                }
                self.infer_expr_int_type(expr).map(RuntimeScalarType::Int)
            }
            Expr::Bool { .. } => Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 8)?)),
            Expr::Number { literal, span } => {
                let value = parse_number_literal(literal, *span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                match value {
                    Value::Int { .. } | Value::UInt { .. } => {
                        Ok(RuntimeScalarType::Int(RuntimeIntType::from_value(&value)?))
                    }
                    Value::Float { bits, .. } => Ok(RuntimeScalarType::Float(bits)),
                    _ => Err(RuntimeGenericLowerError::Unsupported),
                }
            }
            Expr::Ident { name, span } => {
                let binding = self.scopes.get(name).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("unknown identifier '{name}'"),
                        span,
                    ))
                })?;
                if matches!(binding, RuntimeGenericBinding::OwnedPtr { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "owned heap pointers cannot be copied, converted, or used as scalar values",
                        *span,
                    )));
                }
                let Some((_, _, ty)) = binding.as_scalar() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                Ok(ty)
            }
            Expr::FieldAccess { base, field, span } => {
                if let Some((_, field_ty)) =
                    self.lower_struct_slot_field_access(base, field, *span)?
                {
                    return Ok(field_ty);
                }
                Ok(RuntimeScalarType::Int(
                    self.lower_const_struct_field(base, field, *span)?.ty,
                ))
            }
            Expr::Call { name, span, .. } => {
                if matches!(
                    name.as_str(),
                    "runtime_seed"
                        | "runtime_arg_count"
                        | "runtime_monotonic_nanos"
                        | "runtime_wall_time_nanos"
                        | "runtime_process_id"
                        | "runtime_environment_count"
                        | "runtime_stdlib_abi_version"
                ) {
                    return Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 64)?));
                }
                if name == "heap_alloc" {
                    return Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 64)?));
                }
                if name == "heap_free" {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "heap_free() does not return a value",
                        *span,
                    )));
                }
                if name == "runtime_bloom_sbbf_maybe"
                    || name == "runtime_hash_probe_grouped16"
                    || name == "runtime_join_select_adaptive"
                {
                    return Ok(RuntimeScalarType::Int(RuntimeIntType::new(false, 64)?));
                }
                let function = self.functions.get(name).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(unknown_function_diagnostic(name, *span))
                })?;
                let ret_ty = function.return_type.as_ref().ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "expression call requires function with return type",
                        *span,
                    ))
                })?;
                ensure_runtime_generic_scalar_type(ret_ty, *span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)
            }
            Expr::Unary {
                op: UnaryOp::Plus,
                expr,
                ..
            } => self.infer_expr_scalar_type(expr),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                span,
            } => {
                let ty = self.infer_expr_scalar_type(expr)?;
                match ty {
                    RuntimeScalarType::Int(int_ty) => {
                        if !int_ty.signed {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "cannot negate unsigned integer in runtime generic lowering",
                                *span,
                            )));
                        }
                    }
                    RuntimeScalarType::Float(_) => {}
                }
                Ok(ty)
            }
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
                span,
            } => {
                let bool_ty = RuntimeIntType::new(false, 8)?;
                if self.infer_expr_int_type(expr)? != bool_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "logical not expects bool operand in runtime generic lowering",
                        *span,
                    )));
                }
                Ok(RuntimeScalarType::Int(bool_ty))
            }
            Expr::Binary {
                op, left, right, ..
            } => match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                    self.select_expr_scalar_type(left, right)
                }
                BinaryOp::Mod
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::Shl
                | BinaryOp::Shr => Ok(RuntimeScalarType::Int(
                    self.select_expr_int_type(left, right)?,
                )),
                BinaryOp::And | BinaryOp::Or => {
                    let bool_ty = RuntimeIntType::new(false, 8)?;
                    let left_ty = self.infer_expr_int_type(left)?;
                    let right_ty = self.infer_expr_int_type(right)?;
                    if left_ty != bool_ty || right_ty != bool_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "logical operators require bool operands in runtime generic lowering",
                            left.span(),
                        )));
                    }
                    Ok(RuntimeScalarType::Int(bool_ty))
                }
                _ => Err(RuntimeGenericLowerError::Unsupported),
            },
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn select_expr_scalar_type(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> RuntimeGenericLowerResult<RuntimeScalarType> {
        let left_is_lit = runtime_generic_is_number_expr(left);
        let right_is_lit = runtime_generic_is_number_expr(right);
        if left_is_lit && !right_is_lit {
            return self.infer_expr_scalar_type(right);
        }
        if right_is_lit && !left_is_lit {
            return self.infer_expr_scalar_type(left);
        }

        let left_ty = self.infer_expr_scalar_type(left)?;
        let right_ty = self.infer_expr_scalar_type(right)?;
        if left_ty == right_ty {
            Ok(left_ty)
        } else {
            Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "runtime generic type mismatch: {} vs {}",
                    left_ty.display(),
                    right_ty.display()
                )
                .as_str(),
                left.span(),
            )))
        }
    }

    pub(super) fn infer_expr_int_type(
        &self,
        expr: &Expr,
    ) -> RuntimeGenericLowerResult<RuntimeIntType> {
        match expr {
            Expr::Match { value, arms, span } => {
                match self.infer_runtime_match_scalar_type(value, arms, *span)? {
                    RuntimeScalarType::Int(int_ty) => Ok(int_ty),
                    RuntimeScalarType::Float(_) => Err(RuntimeGenericLowerError::Unsupported),
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "char_count" => {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "char_count() expects no arguments",
                        *span,
                    )));
                }
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if self
                    .scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_string)
                    .is_some()
                {
                    RuntimeIntType::new(false, 64)
                } else {
                    Err(RuntimeGenericLowerError::Unsupported)
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if self
                .runtime_user_method_scalar_type(receiver, name, args, *span)?
                .is_some() =>
            {
                match self.runtime_user_method_scalar_type(receiver, name, args, *span)? {
                    Some(RuntimeScalarType::Int(int_ty)) => Ok(int_ty),
                    Some(RuntimeScalarType::Float(_)) | None => {
                        Err(RuntimeGenericLowerError::Unsupported)
                    }
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_ok" | "is_err" | "unwrap_or")
                && self.native_result_ok_scalar_type(receiver).is_some() =>
            {
                match name.as_str() {
                    "is_ok" | "is_err" => {
                        if !args.is_empty() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("Result.{name}() expects no arguments").as_str(),
                                *span,
                            )));
                        }
                        RuntimeIntType::new(false, 8)
                    }
                    "unwrap_or" => {
                        if args.len() != 1 {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "Result.unwrap_or() expects one fallback",
                                *span,
                            )));
                        }
                        match self.native_result_ok_scalar_type(receiver) {
                            Some(RuntimeScalarType::Int(int_ty)) => Ok(int_ty),
                            Some(RuntimeScalarType::Float(_)) | None => {
                                Err(RuntimeGenericLowerError::Unsupported)
                            }
                        }
                    }
                    _ => unreachable!("guarded Result method"),
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_some" | "is_none")
                && self.native_option_struct_layout(receiver).is_some() =>
            {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    )));
                }
                RuntimeIntType::new(false, 8)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "is_some" | "is_none" | "unwrap_or")
                && self.is_native_option_scalar_expr(receiver) =>
            {
                match name.as_str() {
                    "is_some" | "is_none" => {
                        if !args.is_empty() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("{name}() expects no arguments").as_str(),
                                *span,
                            )));
                        }
                        RuntimeIntType::new(false, 8)
                    }
                    "unwrap_or" => {
                        if args.len() != 1 {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "unwrap_or() expects exactly one fallback argument",
                                *span,
                            )));
                        }
                        match self.native_option_scalar_type(receiver) {
                            Some(RuntimeScalarType::Int(int_ty)) => Ok(int_ty),
                            Some(RuntimeScalarType::Float(_)) | None => {
                                Err(RuntimeGenericLowerError::Unsupported)
                            }
                        }
                    }
                    _ => unreachable!("guarded scalar Option method"),
                }
            }
            Expr::Bool { .. } => RuntimeIntType::new(false, 8),
            Expr::Char { .. } => RuntimeIntType::new(false, 32),
            Expr::Number { literal, span } => {
                let value = parse_number_literal(literal, *span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                RuntimeIntType::from_value(&value)
            }
            Expr::Ident { name, span } => {
                let binding = self.scopes.get(name).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("unknown identifier '{name}'"),
                        span,
                    ))
                })?;
                if matches!(binding, RuntimeGenericBinding::OwnedPtr { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "owned heap pointers cannot be copied, converted, or used as scalar values",
                        *span,
                    )));
                }
                let Some((_, _, ty)) = binding.as_scalar() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                match ty {
                    RuntimeScalarType::Int(int_ty) => Ok(int_ty),
                    RuntimeScalarType::Float(_) => Err(RuntimeGenericLowerError::Unsupported),
                }
            }
            Expr::FieldAccess { base, field, span } => {
                if let Some((_, field_ty)) =
                    self.lower_struct_slot_field_access(base, field, *span)?
                {
                    return match field_ty {
                        RuntimeScalarType::Int(field_ty) => Ok(field_ty),
                        RuntimeScalarType::Float(_) => Err(RuntimeGenericLowerError::Unsupported),
                    };
                }
                Ok(self.lower_const_struct_field(base, field, *span)?.ty)
            }
            Expr::Index { base, index, span } => {
                if let Expr::Ident { name, .. } = base.as_ref() {
                    if let Some((_, _, _, _, _, elem_ty)) = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                    {
                        self.infer_expr_int_type(index).map_err(|err| match err {
                            RuntimeGenericLowerError::Unsupported => {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "list index must be integer in runtime generic lowering",
                                    *span,
                                ))
                            }
                            other => other,
                        })?;
                        return match elem_ty {
                            RuntimeScalarType::Int(int_ty) => Ok(int_ty),
                            RuntimeScalarType::Float(_) => {
                                Err(RuntimeGenericLowerError::Unsupported)
                            }
                        };
                    }
                    if let Some((_, _, _, elem_ty, _)) = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_array_slots)
                    {
                        self.infer_expr_int_type(index).map_err(|err| match err {
                            RuntimeGenericLowerError::Unsupported => {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "array index must be integer in runtime generic lowering",
                                    *span,
                                ))
                            }
                            other => other,
                        })?;
                        return Ok(elem_ty);
                    }
                    if let Some((_, _, value_ty)) = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_dict_slots)
                    {
                        if runtime_const_dict_key(index).is_none() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "runtime generic mutable dictionary index must be a string literal",
                                *span,
                            )));
                        }
                        return Ok(value_ty);
                    }
                }
                if let Some(RuntimeOwnedStructListField::Scalar { elem_ty, .. }) =
                    self.owned_struct_list_field(base)
                {
                    self.infer_expr_int_type(index).map_err(|err| match err {
                        RuntimeGenericLowerError::Unsupported => {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "list index must be integer in runtime generic lowering",
                                *span,
                            ))
                        }
                        other => other,
                    })?;
                    return match elem_ty {
                        RuntimeScalarType::Int(int_ty) => Ok(int_ty),
                        RuntimeScalarType::Float(_) => Err(RuntimeGenericLowerError::Unsupported),
                    };
                }
                match self.lower_const_index_access(base, index, *span) {
                    Ok(value) => Ok(value.ty),
                    Err(RuntimeGenericLowerError::Unsupported) => {
                        let Some(RuntimeConstContainer::Array { elems }) =
                            self.resolve_const_container(base)
                        else {
                            return Err(RuntimeGenericLowerError::Unsupported);
                        };
                        self.infer_expr_int_type(index).map_err(|err| match err {
                            RuntimeGenericLowerError::Unsupported => {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "array index must be integer in runtime generic lowering",
                                    *span,
                                ))
                            }
                            other => other,
                        })?;
                        Ok(elems[0].ty)
                    }
                    Err(err) => Err(err),
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "contains" => {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime contains() expects exactly one argument",
                        *span,
                    )));
                }
                if let Expr::Ident { name, .. } = receiver.as_ref()
                    && self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                        .is_some()
                {
                    RuntimeIntType::new(false, 8)
                } else if matches!(
                    self.owned_struct_list_field(receiver),
                    Some(RuntimeOwnedStructListField::Scalar { .. })
                ) {
                    RuntimeIntType::new(false, 8)
                } else {
                    Err(RuntimeGenericLowerError::Unsupported)
                }
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "len" || name == "is_empty" => {
                if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("runtime {}() expects no arguments", name).as_str(),
                        *span,
                    )));
                }
                if let Expr::Ident {
                    name,
                    span: receiver_span,
                } = receiver.as_ref()
                {
                    self.reject_moved_resource(name, *receiver_span)?;
                }
                if name == "is_empty" {
                    return RuntimeIntType::new(false, 8);
                }
                if let Expr::Ident { name, .. } = receiver.as_ref() {
                    if self.scopes.get(name).is_some_and(|binding| {
                        binding.as_owned_list_scalar().is_some()
                            || binding.as_owned_list_struct().is_some()
                            || binding.as_owned_string().is_some()
                            || binding.as_owned_map().is_some()
                    }) {
                        return RuntimeIntType::new(false, 64);
                    }
                    if self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_array_slots)
                        .is_some()
                    {
                        return RuntimeIntType::new(false, 64);
                    }
                    if self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_dict_slots)
                        .is_some()
                    {
                        return RuntimeIntType::new(false, 64);
                    }
                }
                if self.owned_struct_list_field(receiver).is_some() {
                    return RuntimeIntType::new(false, 64);
                }
                match self.resolve_const_container(receiver) {
                    Some(RuntimeConstContainer::Array { .. })
                    | Some(RuntimeConstContainer::Dict { .. }) => RuntimeIntType::new(false, 64),
                    _ => Err(RuntimeGenericLowerError::Unsupported),
                }
            }
            Expr::Call { name, span, .. } => {
                if matches!(
                    name.as_str(),
                    "runtime_seed"
                        | "runtime_arg_count"
                        | "runtime_monotonic_nanos"
                        | "runtime_wall_time_nanos"
                        | "runtime_process_id"
                        | "runtime_environment_count"
                        | "runtime_stdlib_abi_version"
                ) {
                    RuntimeIntType::new(false, 64)
                } else if name == "heap_alloc" {
                    RuntimeIntType::new(false, 64)
                } else if name == "heap_free" {
                    Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "heap_free() does not return a value",
                        *span,
                    )))
                } else if name == "runtime_bloom_sbbf_maybe"
                    || name == "runtime_hash_probe_grouped16"
                    || name == "runtime_join_select_adaptive"
                {
                    RuntimeIntType::new(false, 64)
                } else {
                    let function = self.functions.get(name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(unknown_function_diagnostic(
                            name, *span,
                        ))
                    })?;
                    let ret_ty = function.return_type.as_ref().ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "expression call requires function with return type",
                            *span,
                        ))
                    })?;
                    ensure_runtime_generic_int_type(ret_ty, *span)
                        .map_err(RuntimeGenericLowerError::Diagnostic)
                }
            }
            Expr::Unary {
                op: UnaryOp::Plus,
                expr,
                ..
            } => self.infer_expr_int_type(expr),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                span,
            } => {
                let ty = self.infer_expr_int_type(expr)?;
                if !ty.signed {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "cannot negate unsigned integer in runtime generic lowering",
                        *span,
                    )));
                }
                Ok(ty)
            }
            Expr::Unary {
                op: UnaryOp::Not,
                expr,
                span,
            } => {
                let bool_ty = RuntimeIntType::new(false, 8)?;
                let ty = self.infer_expr_int_type(expr)?;
                if ty != bool_ty {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "logical not expects bool operand in runtime generic lowering",
                        *span,
                    )));
                }
                Ok(bool_ty)
            }
            Expr::Binary {
                op, left, right, ..
            } => match op {
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::Shl
                | BinaryOp::Shr => self.select_expr_int_type(left, right),
                BinaryOp::And | BinaryOp::Or => {
                    let bool_ty = RuntimeIntType::new(false, 8)?;
                    let left_ty = self.infer_expr_int_type(left)?;
                    let right_ty = self.infer_expr_int_type(right)?;
                    if left_ty != bool_ty || right_ty != bool_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "logical operators require bool operands in runtime generic lowering",
                            left.span(),
                        )));
                    }
                    Ok(bool_ty)
                }
                _ => Err(RuntimeGenericLowerError::Unsupported),
            },
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn select_expr_int_type(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> RuntimeGenericLowerResult<RuntimeIntType> {
        let left_is_lit = runtime_generic_is_number_expr(left);
        let right_is_lit = runtime_generic_is_number_expr(right);
        if left_is_lit && !right_is_lit {
            return self.infer_expr_int_type(right);
        }
        if right_is_lit && !left_is_lit {
            return self.infer_expr_int_type(left);
        }

        let left_ty = self.infer_expr_int_type(left)?;
        let right_ty = self.infer_expr_int_type(right)?;
        if left_ty == right_ty {
            Ok(left_ty)
        } else {
            Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "runtime generic integer type mismatch: {} vs {}",
                    left_ty.display(),
                    right_ty.display()
                )
                .as_str(),
                right.span(),
            )))
        }
    }

    pub(super) fn lower_cmp_operands(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> RuntimeGenericLowerResult<(RuntimeCmpOp, RuntimeOperand, RuntimeOperand)> {
        let cmp_ty = self.select_expr_int_type(left, right)?;
        let cmp = cmp_ty
            .cmp_from_binary(op)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let lhs = self.lower_expr_as_type(left, cmp_ty)?;
        let rhs = self.lower_expr_as_type(right, cmp_ty)?;
        Ok((cmp, lhs, rhs))
    }

    pub(super) fn try_lower_float_comparison(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> RuntimeGenericLowerResult<Option<usize>> {
        if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            return Ok(None);
        }
        let scalar_ty = match self.select_expr_scalar_type(left, right) {
            Ok(scalar_ty) => scalar_ty,
            Err(RuntimeGenericLowerError::Unsupported) => return Ok(None),
            Err(RuntimeGenericLowerError::Diagnostic(_)) => return Ok(None),
        };
        let RuntimeScalarType::Float(bits) = scalar_ty else {
            return Ok(None);
        };
        let lhs = self.lower_expr_as_float(left, bits)?;
        let rhs = self.lower_expr_as_float(right, bits)?;
        let equal = self.emit_float_eq(lhs, rhs, bits)?;
        if op == BinaryOp::Eq {
            return Ok(Some(equal));
        }
        let result = self.alloc_slot();
        self.emit(RuntimeInstr::Cmp {
            dst: result,
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(equal),
            rhs: RuntimeOperand::Imm(0),
        });
        Ok(Some(result))
    }
}
