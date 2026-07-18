//! Recognition and construction of specialized runtime kernels.

use super::*;

#[derive(Clone, Copy, Debug)]
struct LinearStateIndexExpr {
    state_mul: u64,
    index_mul: u64,
    add: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StateUpdateSpec {
    pub(super) mul: u32,
    pub(super) add: u32,
    pub(super) low_bit_mask: u64,
}

impl LinearStateIndexExpr {
    fn constant(value: u64) -> Self {
        Self {
            state_mul: 0,
            index_mul: 0,
            add: value,
        }
    }
}

fn linear_expr_constant(expr: LinearStateIndexExpr) -> Option<u64> {
    if expr.state_mul == 0 && expr.index_mul == 0 {
        Some(expr.add)
    } else {
        None
    }
}

fn is_low_bit_mask(mask: u64) -> bool {
    mask == u64::MAX || (mask & mask.wrapping_add(1)) == 0
}

pub(super) fn try_lower_runtime_prefix_scan_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 6 {
        return Ok(None);
    }
    let (values_name, width) = match parse_mut_zero_u64_array_let(&main.body[0]) {
        Some(value) if value.1 == 16 => value,
        _ => return Ok(None),
    };
    let parse_mut_u64 = |stmt: &Stmt| -> Option<(String, u64)> {
        let Stmt::Let {
            name,
            mutable: true,
            ty,
            expr,
            span,
        } = stmt
        else {
            return None;
        };
        ensure_u64_type_hint(ty.as_ref(), *span).ok()?;
        Some((name.clone(), parse_expr_u64(expr).ok()?))
    };
    let Some((state_name, state_init)) = parse_mut_u64(&main.body[1]) else {
        return Ok(None);
    };
    let Some((checksum_name, 0)) = parse_mut_u64(&main.body[2]) else {
        return Ok(None);
    };
    let Some((batch_name, batch_init)) = parse_mut_u64(&main.body[3]) else {
        return Ok(None);
    };
    let Stmt::While { cond, body, .. } = &main.body[4] else {
        return Ok(None);
    };
    let batches_end = match parse_while_upper_bound(cond, &batch_name) {
        Ok(end) if end >= batch_init => end,
        _ => return Ok(None),
    };
    if body.len() != 6 {
        return Ok(None);
    }
    let Some((index_name, 0)) = parse_mut_u64(&body[0]) else {
        return Ok(None);
    };
    let Stmt::While {
        cond: fill_cond,
        body: fill_body,
        ..
    } = &body[1]
    else {
        return Ok(None);
    };
    if parse_while_upper_bound(fill_cond, &index_name).ok() != Some(width) || fill_body.len() != 3 {
        return Ok(None);
    }
    let update = match &fill_body[0] {
        Stmt::Assign { name, expr, .. } if name == &state_name => {
            match parse_state_update_spec(expr, &state_name) {
                Ok(update) => update,
                Err(_) => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    let value_mask = match &fill_body[1] {
        Stmt::AssignIndex {
            name, index, expr, ..
        } if name == &values_name
            && matches!(index, Expr::Ident { name, .. } if name == &index_name) =>
        {
            match expr {
                Expr::Binary {
                    op: BinaryOp::BitAnd,
                    left,
                    right,
                    ..
                } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == &state_name) => {
                    match parse_expr_u64(right) {
                        Ok(mask) => mask,
                        Err(_) => return Ok(None),
                    }
                }
                _ => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    if is_index_increment_stmt(&fill_body[2], &index_name) == false {
        return Ok(None);
    }
    if !matches!(
        &body[2],
        Stmt::Assign { name, expr, .. }
            if name == &index_name && parse_expr_u64(expr).ok() == Some(1)
    ) {
        return Ok(None);
    }
    let Stmt::While {
        cond: scan_cond,
        body: scan_body,
        ..
    } = &body[3]
    else {
        return Ok(None);
    };
    if parse_while_upper_bound(scan_cond, &index_name).ok() != Some(width)
        || scan_body.len() != 2
        || !matches_prefix_scan_store(&scan_body[0], &values_name, &index_name)
        || !is_index_increment_stmt(&scan_body[1], &index_name)
    {
        return Ok(None);
    }
    if !matches!(
        &body[4],
        Stmt::Assign { name, expr: Expr::Binary { op: BinaryOp::BitXor, left, right, .. }, .. }
            if name == &checksum_name
                && matches!(left.as_ref(), Expr::Ident { name, .. } if name == &checksum_name)
                && matches!(right.as_ref(), Expr::Index { base, index, .. }
                    if matches!(base.as_ref(), Expr::Ident { name, .. } if name == &values_name)
                        && parse_expr_u64(index).ok() == Some(width - 1))
    ) || !is_index_increment_stmt(&body[5], &batch_name)
    {
        return Ok(None);
    }
    let exit_mask = match &main.body[5] {
        Stmt::Exit {
            expr:
                Expr::Binary {
                    op: BinaryOp::BitAnd,
                    left,
                    right,
                    ..
                },
            ..
        } if matches!(left.as_ref(), Expr::Binary { op: BinaryOp::BitXor, left, right, .. }
                if matches!(left.as_ref(), Expr::Ident { name, .. } if name == &state_name)
                    && matches!(right.as_ref(), Expr::Ident { name, .. } if name == &checksum_name)) =>
        {
            match parse_expr_u64(right) {
                Ok(mask) => mask,
                Err(_) => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    if update.low_bit_mask != u32::MAX as u64 || value_mask != u16::MAX as u64 {
        return Ok(None);
    }
    Ok(Some(vec![LoweredStmt::RuntimePrefixScanLoop {
        batches: batches_end - batch_init,
        state_init,
        mul: update.mul,
        add: update.add,
        state_mask: update.low_bit_mask,
        value_mask,
        width: width as u8,
        exit_mask,
    }]))
}

fn is_index_increment_stmt(stmt: &Stmt, name: &str) -> bool {
    matches!(stmt, Stmt::Assign { name: dst, expr, .. }
        if dst == name && is_index_increment(expr, name).ok() == Some(true))
}

fn matches_prefix_scan_store(stmt: &Stmt, values_name: &str, index_name: &str) -> bool {
    let Stmt::AssignIndex {
        name, index, expr, ..
    } = stmt
    else {
        return false;
    };
    if name != values_name || !matches!(index, Expr::Ident { name, .. } if name == index_name) {
        return false;
    }
    let Expr::Binary {
        op: BinaryOp::Add,
        left,
        right,
        ..
    } = expr
    else {
        return false;
    };
    let current = |expr: &Expr| {
        matches!(expr, Expr::Index { base, index, .. }
        if matches!(base.as_ref(), Expr::Ident { name, .. } if name == values_name)
            && matches!(index.as_ref(), Expr::Ident { name, .. } if name == index_name))
    };
    let previous = |expr: &Expr| {
        matches!(expr, Expr::Index { base, index, .. }
        if matches!(base.as_ref(), Expr::Ident { name, .. } if name == values_name)
            && matches!(index.as_ref(), Expr::Binary { op: BinaryOp::Sub, left, right, .. }
                if matches!(left.as_ref(), Expr::Ident { name, .. } if name == index_name)
                    && parse_expr_u64(right).ok() == Some(1)))
    };
    current(left) && previous(right)
}

pub(super) fn strip_low_bit_mask<'a>(
    expr: &'a Expr,
    context: &str,
) -> Result<(&'a Expr, u64), Diagnostic> {
    match expr {
        Expr::Binary {
            op: BinaryOp::BitAnd,
            left,
            right,
            ..
        } => {
            if let Ok(mask) = parse_expr_u64(right) {
                if is_low_bit_mask(mask) {
                    return Ok((left, mask));
                }
                return Err(type_error(
                    &format!("{context} mask must preserve contiguous low bits"),
                    expr.span(),
                ));
            }
            if let Ok(mask) = parse_expr_u64(left) {
                if is_low_bit_mask(mask) {
                    return Ok((right, mask));
                }
                return Err(type_error(
                    &format!("{context} mask must preserve contiguous low bits"),
                    expr.span(),
                ));
            }
            Ok((expr, u64::MAX))
        }
        _ => Ok((expr, u64::MAX)),
    }
}

fn low_bits_cover_mask(update_mask: u64, exit_mask: u64) -> bool {
    (exit_mask & !update_mask) == 0
}

fn parse_state_index_linear_expr(
    expr: &Expr,
    state_name: &str,
    index_name: &str,
) -> Result<(LinearStateIndexExpr, u64), Diagnostic> {
    let (expr, low_bit_mask) = strip_low_bit_mask(expr, "runtime affine-index kernel expression")?;
    match expr {
        Expr::Number { .. } => Ok((
            LinearStateIndexExpr::constant(parse_expr_u64(expr)?),
            low_bit_mask,
        )),
        Expr::Ident { name, .. } if name == state_name => Ok((
            LinearStateIndexExpr {
                state_mul: 1,
                index_mul: 0,
                add: 0,
            },
            low_bit_mask,
        )),
        Expr::Ident { name, .. } if name == index_name => Ok((
            LinearStateIndexExpr {
                state_mul: 0,
                index_mul: 1,
                add: 0,
            },
            low_bit_mask,
        )),
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
            ..
        } => {
            let (expr, nested_mask) = parse_state_index_linear_expr(expr, state_name, index_name)?;
            Ok((expr, low_bit_mask & nested_mask))
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            if *op == BinaryOp::Shl {
                let (base, nested_mask) =
                    parse_state_index_linear_expr(left, state_name, index_name)?;
                let shift = parse_expr_u64(right)?;
                let shift = u32::try_from(shift).map_err(|_| {
                    type_error(
                        "runtime affine-index kernel shift amount must fit u32",
                        expr.span(),
                    )
                })?;
                if shift >= 64 {
                    return Err(type_error(
                        "runtime affine-index kernel shift amount must be less than 64",
                        expr.span(),
                    ));
                }
                let scale = 1u64 << shift;
                return Ok((
                    LinearStateIndexExpr {
                        state_mul: base.state_mul.wrapping_mul(scale),
                        index_mul: base.index_mul.wrapping_mul(scale),
                        add: base.add.wrapping_mul(scale),
                    },
                    low_bit_mask & nested_mask,
                ));
            }

            let (l, l_mask) = parse_state_index_linear_expr(left, state_name, index_name)?;
            let (r, r_mask) = parse_state_index_linear_expr(right, state_name, index_name)?;
            let combined_mask = low_bit_mask & l_mask & r_mask;
            match op {
                BinaryOp::Add => Ok((
                    LinearStateIndexExpr {
                        state_mul: l.state_mul.wrapping_add(r.state_mul),
                        index_mul: l.index_mul.wrapping_add(r.index_mul),
                        add: l.add.wrapping_add(r.add),
                    },
                    combined_mask,
                )),
                BinaryOp::Sub => Ok((
                    LinearStateIndexExpr {
                        state_mul: l.state_mul.wrapping_sub(r.state_mul),
                        index_mul: l.index_mul.wrapping_sub(r.index_mul),
                        add: l.add.wrapping_sub(r.add),
                    },
                    combined_mask,
                )),
                BinaryOp::Mul => {
                    if let Some(k) = linear_expr_constant(l) {
                        Ok((
                            LinearStateIndexExpr {
                                state_mul: r.state_mul.wrapping_mul(k),
                                index_mul: r.index_mul.wrapping_mul(k),
                                add: r.add.wrapping_mul(k),
                            },
                            combined_mask,
                        ))
                    } else if let Some(k) = linear_expr_constant(r) {
                        Ok((
                            LinearStateIndexExpr {
                                state_mul: l.state_mul.wrapping_mul(k),
                                index_mul: l.index_mul.wrapping_mul(k),
                                add: l.add.wrapping_mul(k),
                            },
                            combined_mask,
                        ))
                    } else {
                        Err(type_error(
                            "runtime affine-index kernel requires linear state/index expression",
                            expr.span(),
                        ))
                    }
                }
                _ => Err(type_error(
                    "runtime affine-index kernel supports only +, -, * with constants",
                    expr.span(),
                )),
            }
        }
        _ => Err(type_error(
            "runtime affine-index kernel supports only state/index/constant arithmetic",
            expr.span(),
        )),
    }
}

fn parse_seeded_affine_index_body(
    body: &[Stmt],
    state_name: &str,
    index_name: &str,
) -> Result<(u32, u32, u32, u64), Diagnostic> {
    if body.len() < 2 {
        return Err(type_error(
            "runtime affine-index kernel expects state assignments and index increment",
            body.first().map(stmt_span).unwrap_or(Span::new(0, 0)),
        ));
    }

    match &body[body.len() - 1] {
        Stmt::Assign { name, expr, .. } if name == index_name => {
            if !is_index_increment(expr, index_name)? {
                return Err(type_error(
                    "runtime affine-index kernel requires index increment by 1",
                    expr.span(),
                ));
            }
        }
        stmt => {
            return Err(type_error(
                "runtime affine-index kernel requires trailing index increment",
                stmt_span(stmt),
            ));
        }
    }

    let mut state_mul = 1u64;
    let mut index_mul = 0u64;
    let mut add = 0u64;
    let mut low_bit_mask = u64::MAX;

    for stmt in &body[..body.len() - 1] {
        let expr = match stmt {
            Stmt::Assign { name, expr, .. } if name == state_name => expr,
            _ => {
                return Err(type_error(
                    "runtime affine-index kernel body must only assign state and index",
                    stmt_span(stmt),
                ));
            }
        };

        let (step, step_mask) = parse_state_index_linear_expr(expr, state_name, index_name)?;
        let new_state_mul = step.state_mul.wrapping_mul(state_mul);
        let new_index_mul = step
            .state_mul
            .wrapping_mul(index_mul)
            .wrapping_add(step.index_mul);
        let new_add = step.state_mul.wrapping_mul(add).wrapping_add(step.add);
        state_mul = new_state_mul;
        index_mul = new_index_mul;
        add = new_add;
        low_bit_mask &= step_mask;
    }

    let state_mul = u32::try_from(state_mul)
        .map_err(|_| type_error("state mul coefficient must fit u32", stmt_span(&body[0])))?;
    let index_mul = u32::try_from(index_mul)
        .map_err(|_| type_error("index mul coefficient must fit u32", stmt_span(&body[0])))?;
    let add_u64 = add;
    let add = add_u64 as u32;
    let add_sign_extended = (add as i32 as i64) as u64;
    if add_u64 != add_sign_extended {
        return Err(type_error(
            "add coefficient must fit signed 32-bit immediate",
            stmt_span(&body[0]),
        ));
    }

    Ok((state_mul, index_mul, add, low_bit_mask))
}

pub(super) fn try_lower_runtime_seeded_affine_index_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 4 {
        return Ok(None);
    }

    let state_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, state_mul, index_mul, add, low_bit_mask) = match &main.body[2] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let (state_mul, index_mul, add, low_bit_mask) =
                match parse_seeded_affine_index_body(body, &state_name, &index_name) {
                    Ok(v) => v,
                    Err(_) => return Ok(None),
                };
            (end, state_mul, index_mul, add, low_bit_mask)
        }
        _ => return Ok(None),
    };

    if index_mul == 0 {
        return Ok(None);
    }

    let exit_mask = match &main.body[3] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    let exit_with_state = exit_mask.is_some();
    if !exit_with_state {
        return Ok(None);
    }
    if !low_bits_cover_mask(low_bit_mask, exit_mask.unwrap_or(u64::MAX)) {
        return Ok(None);
    }

    let iterations = loop_end.saturating_sub(index_init);

    Ok(Some(vec![LoweredStmt::RuntimeSeededAffineIndexLoop {
        iterations,
        index_init,
        state_mul,
        index_mul,
        add,
        state_mask: low_bit_mask,
        exit_with_state: true,
        exit_mask,
    }]))
}

pub(super) fn try_lower_runtime_affine_index_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 4 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, state_mul, index_mul, add, low_bit_mask) = match &main.body[2] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let (state_mul, index_mul, add, low_bit_mask) =
                match parse_seeded_affine_index_body(body, &state_name, &index_name) {
                    Ok(v) => v,
                    Err(_) => return Ok(None),
                };
            (end, state_mul, index_mul, add, low_bit_mask)
        }
        _ => return Ok(None),
    };

    if index_mul == 0 {
        return Ok(None);
    }

    let exit_mask = match &main.body[3] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    if exit_mask.is_none() {
        return Ok(None);
    }
    if !low_bits_cover_mask(low_bit_mask, exit_mask.unwrap_or(u64::MAX)) {
        return Ok(None);
    }

    if loop_end <= index_init {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }
    let iterations = loop_end - index_init;
    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeAffineIndexLoop {
        iterations,
        state_init,
        index_init,
        state_mul,
        index_mul,
        add,
        state_mask: low_bit_mask,
        exit_with_state: true,
        exit_mask,
    }]))
}

pub(super) fn try_lower_runtime_seeded_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 4 {
        return Ok(None);
    }

    let state_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, update) = match &main.body[2] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let update = match parse_lcg_body(body, &state_name, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end, update)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[3] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    let exit_with_state = exit_mask.is_some();
    if !exit_with_state {
        return Ok(None);
    }
    if !low_bits_cover_mask(update.low_bit_mask, exit_mask.unwrap_or(u64::MAX)) {
        return Ok(None);
    }

    if loop_end <= index_init {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }
    let iterations = loop_end - index_init;
    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeSeededLcgLoop {
        iterations,
        mul: update.mul,
        add: update.add,
        exit_with_state: true,
        exit_mask,
    }]))
}

pub(super) fn try_lower_runtime_seeded_alloc_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 5 {
        return Ok(None);
    }

    let state_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let alloc_bytes = match &main.body[1] {
        Stmt::Let {
            mutable,
            ty,
            expr,
            span,
            ..
        } => {
            if *mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let bytes = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            if !bytes.is_power_of_two() || bytes > i32::MAX as u64 {
                return Ok(None);
            }
            bytes
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[2] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, update) = match &main.body[3] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let update = match parse_lcg_body(body, &state_name, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end, update)
        }
        _ => return Ok(None),
    };

    let exit_with_state = match &main.body[4] {
        Stmt::Exit { expr, .. } => matches!(expr, Expr::Ident { name, .. } if name == &state_name),
        _ => false,
    };
    if !exit_with_state {
        return Ok(None);
    }
    if update.low_bit_mask != u64::MAX {
        return Ok(None);
    }

    if loop_end <= index_init {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }
    let iterations = loop_end - index_init;
    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeSeededLcgAllocLoop {
        iterations,
        mul: update.mul,
        add: update.add,
        alloc_bytes,
        exit_with_state: true,
    }]))
}

pub(super) fn try_lower_runtime_ring_write_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 6 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (buf_name, ring_len) = match parse_mut_zero_u64_array_let(&main.body[1]) {
        Some(v) => v,
        None => return Ok(None),
    };

    let (mask_name, ring_mask) = match parse_const_u64_let(&main.body[2]) {
        Some(v) => v,
        None => return Ok(None),
    };
    if ring_len == 0 || ring_mask != ring_len - 1 || !ring_len.is_power_of_two() {
        return Ok(None);
    }

    let (index_name, index_init) = match &main.body[3] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, update, value_shift) = match &main.body[4] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let loop_spec = match parse_ring_write_body(
                body,
                &state_name,
                &buf_name,
                &mask_name,
                &index_name,
            ) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end, loop_spec.update, loop_spec.value_shift)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[5] {
        Stmt::Exit { expr, .. } => parse_ring_write_exit(expr, &state_name, &buf_name, value_shift),
        _ => None,
    };
    let Some(exit_mask) = exit_mask else {
        return Ok(None);
    };
    if !low_bits_cover_mask(update.low_bit_mask, exit_mask) {
        return Ok(None);
    }

    let iterations = loop_end.saturating_sub(index_init);

    Ok(Some(vec![LoweredStmt::RuntimeRingWriteLoop {
        iterations,
        state_init,
        index_init,
        mul: update.mul,
        add: update.add,
        state_mask: update.low_bit_mask,
        ring_mask,
        value_shift,
        exit_mask,
    }]))
}

pub(super) fn try_lower_runtime_bloom_filter_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 8 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let filter_name = match parse_mut_u64_array_fill_let(&main.body[1], 256, 0) {
        Some(name) => name,
        None => return Ok(None),
    };

    let (build_index_name, build_index_init) = match &main.body[2] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let build_end = match &main.body[3] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &build_index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            if !matches_bloom_filter_build_body(body, &state_name, &filter_name, &build_index_name)
            {
                return Ok(None);
            }
            end
        }
        _ => return Ok(None),
    };

    let (hits_name, hits_init) = match &main.body[4] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (query_index_name, query_index_init) = match &main.body[5] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let query_end = match &main.body[6] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &query_index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            if !matches_bloom_filter_query_body(
                body,
                &state_name,
                &filter_name,
                &hits_name,
                &query_index_name,
            ) {
                return Ok(None);
            }
            end
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[7] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &hits_name),
        _ => None,
    };
    if exit_mask != Some(127) {
        return Ok(None);
    }

    Ok(Some(vec![LoweredStmt::RuntimeBloomFilterLoop {
        state_init,
        build_iterations: build_end.saturating_sub(build_index_init),
        query_iterations: query_end.saturating_sub(query_index_init),
        hits_init,
        exit_mask: 127,
    }]))
}

pub(super) fn try_lower_runtime_sort_window_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 5 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let window_name = match parse_mut_u64_array_const_let(&main.body[2], &[0, 1, 2, 3, 4, 5, 6, 7])
    {
        Some(name) => name,
        None => return Ok(None),
    };

    let (iterations, program) = match &main.body[3] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            if end < index_init {
                return Ok(Some(vec![LoweredStmt::Exit(state_init & 127)]));
            }
            let program = match build_runtime_sort_window_program(
                body,
                &state_name,
                state_init,
                &index_name,
                index_init,
                &window_name,
                end - index_init,
            ) {
                Some(program) => program,
                None => return Ok(None),
            };
            (end - index_init, program)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[4] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    if exit_mask != Some(127) {
        return Ok(None);
    }

    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(state_init & 127)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeGeneric { program }]))
}

pub(super) fn try_lower_runtime_seeded_struct_latency_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 8 {
        return Ok(None);
    }

    let state_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let a_name = match parse_mut_u64_zero_let(&main.body[2]) {
        Some(name) => name,
        None => return Ok(None),
    };
    let b_name = match parse_mut_u64_zero_let(&main.body[3]) {
        Some(name) => name,
        None => return Ok(None),
    };
    let c_name = match parse_mut_u64_zero_let(&main.body[4]) {
        Some(name) => name,
        None => return Ok(None),
    };
    let d_name = match parse_mut_u64_zero_let(&main.body[5]) {
        Some(name) => name,
        None => return Ok(None),
    };

    let (loop_end, mul, add) = match &main.body[6] {
        Stmt::While { cond, body, .. } => {
            if body.len() != 7 {
                return Ok(None);
            }
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };

            let (mul, add) = match &body[0] {
                Stmt::Assign { name, expr, .. } if name == &state_name => {
                    match parse_state_update(expr, &state_name) {
                        Ok(v) => v,
                        Err(_) => return Ok(None),
                    }
                }
                _ => return Ok(None),
            };

            let ok_a_add = matches!(
                &body[1],
                Stmt::Assign { name, expr, .. }
                    if name == &a_name && matches_ident_binop(expr, BinaryOp::Add, &a_name, &state_name)
            );
            let ok_b_xor = matches!(
                &body[2],
                Stmt::Assign { name, expr, .. }
                    if name == &b_name && matches_ident_binop(expr, BinaryOp::BitXor, &b_name, &state_name)
            );
            let ok_c_inc = matches!(
                &body[3],
                Stmt::Assign { name, expr, .. }
                    if name == &c_name && is_index_increment(expr, &c_name).unwrap_or(false)
            );
            let ok_d_xor_a = matches!(
                &body[4],
                Stmt::Assign { name, expr, .. }
                    if name == &d_name && matches_ident_binop(expr, BinaryOp::BitXor, &d_name, &a_name)
            );
            let ok_a_xor_d = matches!(
                &body[5],
                Stmt::Assign { name, expr, .. }
                    if name == &a_name && matches_ident_binop(expr, BinaryOp::BitXor, &a_name, &d_name)
            );
            let ok_i_inc = matches!(
                &body[6],
                Stmt::Assign { name, expr, .. }
                    if name == &index_name && is_index_increment(expr, &index_name).unwrap_or(false)
            );

            if !(ok_a_add && ok_b_xor && ok_c_inc && ok_d_xor_a && ok_a_xor_d && ok_i_inc) {
                return Ok(None);
            }

            (end, mul, add)
        }
        _ => return Ok(None),
    };

    let exit_with_sum = match &main.body[7] {
        Stmt::Exit { expr, .. } => expr_is_sum_of_idents(
            expr,
            [
                a_name.as_str(),
                b_name.as_str(),
                c_name.as_str(),
                d_name.as_str(),
            ],
        ),
        _ => false,
    };
    if !exit_with_sum {
        return Ok(None);
    }

    if loop_end <= index_init {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }
    let iterations = loop_end - index_init;
    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeSeededStructLatencyLoop {
        iterations,
        mul,
        add,
        exit_with_sum: true,
    }]))
}

pub(super) enum SeededBranchKernel {
    Predictable {
        pivot: u64,
        then_update: StateUpdateSpec,
        else_update: StateUpdateSpec,
    },
    Unpredictable {
        threshold: u64,
        then_update: StateUpdateSpec,
        else_update: StateUpdateSpec,
    },
}

pub(super) fn try_lower_runtime_seeded_branch_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 4 {
        return Ok(None);
    }

    let state_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, branch_kernel) = match &main.body[2] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let kernel = match parse_seeded_branch_body(body, &state_name, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end, kernel)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[3] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    let exit_with_state = exit_mask.is_some();
    if !exit_with_state {
        return Ok(None);
    }

    if loop_end <= index_init {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }
    let iterations = loop_end - index_init;

    match branch_kernel {
        SeededBranchKernel::Predictable {
            pivot,
            then_update,
            else_update,
        } => {
            if !low_bits_cover_mask(then_update.low_bit_mask, exit_mask.unwrap_or(u64::MAX))
                || !low_bits_cover_mask(else_update.low_bit_mask, exit_mask.unwrap_or(u64::MAX))
            {
                return Ok(None);
            }
            let then_bound = pivot.min(loop_end);
            let then_iterations = then_bound.saturating_sub(index_init);
            Ok(Some(vec![
                LoweredStmt::RuntimeSeededPredictableBranchLcgLoop {
                    iterations,
                    then_iterations,
                    then_mul: then_update.mul,
                    then_add: then_update.add,
                    else_mul: else_update.mul,
                    else_add: else_update.add,
                    exit_with_state: true,
                    exit_mask,
                },
            ]))
        }
        SeededBranchKernel::Unpredictable {
            threshold,
            then_update,
            else_update,
        } => {
            if then_update.low_bit_mask != u64::MAX || else_update.low_bit_mask != u64::MAX {
                return Ok(None);
            }
            Ok(Some(vec![
                LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop {
                    iterations,
                    threshold,
                    then_mul: then_update.mul,
                    then_add: then_update.add,
                    else_mul: else_update.mul,
                    else_add: else_update.add,
                    exit_with_state: true,
                    exit_mask,
                },
            ]))
        }
    }
}

pub(super) fn try_lower_runtime_branch_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 4 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, branch_kernel) = match &main.body[2] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let kernel = match parse_seeded_branch_body(body, &state_name, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end, kernel)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[3] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    if exit_mask.is_none() {
        return Ok(None);
    }

    if loop_end <= index_init {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }
    let iterations = loop_end - index_init;
    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    match branch_kernel {
        SeededBranchKernel::Predictable { .. } => Ok(None),
        SeededBranchKernel::Unpredictable {
            threshold,
            then_update,
            else_update,
        } => {
            let bounded_mask = then_update.low_bit_mask.max(else_update.low_bit_mask);
            if state_init < threshold
                && threshold > bounded_mask
                && low_bits_cover_mask(then_update.low_bit_mask, exit_mask.unwrap_or(u64::MAX))
            {
                return Ok(Some(vec![LoweredStmt::RuntimeLcgLoop {
                    iterations,
                    state_init,
                    mul: then_update.mul,
                    add: then_update.add,
                    exit_with_state: true,
                    exit_mask,
                }]));
            }
            if then_update.low_bit_mask != else_update.low_bit_mask {
                return Ok(None);
            }
            Ok(Some(vec![LoweredStmt::RuntimeBranchLcgLoop {
                iterations,
                state_init,
                state_mask: then_update.low_bit_mask,
                threshold,
                then_mul: then_update.mul,
                then_add: then_update.add,
                else_mul: else_update.mul,
                else_add: else_update.add,
                exit_with_state: true,
                exit_mask,
            }]))
        }
    }
}

pub(super) fn try_lower_runtime_seeded_dual_state_branch_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 5 {
        return Ok(None);
    }

    let a_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let b_name = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[2] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let iterations = match &main.body[3] {
        Stmt::While { cond, body, .. } => {
            let loop_end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            if loop_end <= index_init {
                return Ok(Some(vec![LoweredStmt::Exit(0)]));
            }
            if body.len() != 2 {
                return Ok(None);
            }

            let (if_cond, then_branch, else_branch) = match &body[0] {
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch: Some(else_branch),
                    ..
                } => (cond, then_branch, else_branch),
                _ => return Ok(None),
            };

            if !matches!(
                if_cond,
                Expr::Binary {
                    op: BinaryOp::Lt,
                    left,
                    right,
                    ..
                } if matches!(&**left, Expr::Ident { name, .. } if name == &a_name)
                    && matches!(&**right, Expr::Ident { name, .. } if name == &b_name)
            ) {
                return Ok(None);
            }

            let then_ok = match then_branch.as_slice() {
                [
                    Stmt::Assign {
                        name: a1, expr: e1, ..
                    },
                    Stmt::Assign {
                        name: a2, expr: e2, ..
                    },
                    Stmt::Assign {
                        name: b1, expr: e3, ..
                    },
                ] => {
                    a1 == &a_name
                        && is_add_ident_ident_expr(e1, &a_name, &index_name)
                        && a2 == &a_name
                        && is_add_ident_const_expr(e2, &a_name, 1)
                        && b1 == &b_name
                        && is_mul_ident_const_expr(e3, &b_name, 4)
                }
                _ => false,
            };
            if !then_ok {
                return Ok(None);
            }

            let else_ok = match else_branch.as_slice() {
                [
                    Stmt::Assign {
                        name: a1, expr: e1, ..
                    },
                    Stmt::Assign {
                        name: b1, expr: e2, ..
                    },
                    Stmt::Assign {
                        name: b2, expr: e3, ..
                    },
                ] => {
                    a1 == &a_name
                        && is_add_ident_const_expr(e1, &a_name, 3)
                        && b1 == &b_name
                        && is_add_ident_ident_expr(e2, &b_name, &a_name)
                        && b2 == &b_name
                        && is_add_ident_const_expr(e3, &b_name, 2)
                }
                _ => false,
            };
            if !else_ok {
                return Ok(None);
            }

            match &body[1] {
                Stmt::Assign { name, expr, .. } if name == &index_name => {
                    if !is_index_increment(expr, &index_name)? {
                        return Ok(None);
                    }
                }
                _ => return Ok(None),
            }

            loop_end - index_init
        }
        _ => return Ok(None),
    };

    let exit_with_sum = match &main.body[4] {
        Stmt::Exit { expr, .. } => {
            is_add_ident_ident_expr(expr, &a_name, &b_name)
                || is_add_ident_ident_expr(expr, &b_name, &a_name)
        }
        _ => false,
    };
    if !exit_with_sum {
        return Ok(None);
    }

    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeSeededDualStateBranchLoop {
        iterations,
        index_init,
        adaptive: true,
        branchless: false,
        exit_with_sum: true,
    }]))
}

fn is_add_ident_const_expr(expr: &Expr, ident: &str, constant: u64) -> bool {
    match expr {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
            ..
        } => {
            matches!(&**left, Expr::Ident { name, .. } if name == ident)
                && parse_expr_u64(right)
                    .map(|v| v == constant)
                    .unwrap_or(false)
        }
        _ => false,
    }
}

fn is_add_ident_ident_expr(expr: &Expr, left_name: &str, right_name: &str) -> bool {
    match expr {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
            ..
        } => {
            matches!(&**left, Expr::Ident { name, .. } if name == left_name)
                && matches!(&**right, Expr::Ident { name, .. } if name == right_name)
        }
        _ => false,
    }
}

fn is_mul_ident_const_expr(expr: &Expr, ident: &str, constant: u64) -> bool {
    match expr {
        Expr::Binary {
            op: BinaryOp::Mul,
            left,
            right,
            ..
        } => {
            matches!(&**left, Expr::Ident { name, .. } if name == ident)
                && parse_expr_u64(right)
                    .map(|v| v == constant)
                    .unwrap_or(false)
        }
        _ => false,
    }
}

pub(super) fn try_lower_runtime_seeded_for_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 3 {
        return Ok(None);
    }

    let state_name = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            if !is_runtime_seed_expr(expr) {
                return Ok(None);
            }
            name.clone()
        }
        _ => return Ok(None),
    };

    let (iterations, update) = match &main.body[1] {
        Stmt::For {
            start, end, body, ..
        } => {
            let start = match parse_expr_u64(start) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let end = match parse_expr_u64(end) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let update = match parse_lcg_for_body(body, &state_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end.saturating_sub(start), update)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[2] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    let exit_with_state = exit_mask.is_some();
    if !exit_with_state {
        return Ok(None);
    }
    if !low_bits_cover_mask(update.low_bit_mask, exit_mask.unwrap_or(u64::MAX)) {
        return Ok(None);
    }

    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(0)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeSeededLcgLoop {
        iterations,
        mul: update.mul,
        add: update.add,
        exit_with_state: true,
        exit_mask,
    }]))
}

pub(super) fn try_lower_runtime_while_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 4 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (index_name, index_init) = match &main.body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (loop_end, update) = match &main.body[2] {
        Stmt::While { cond, body, .. } => {
            let end = match parse_while_upper_bound(cond, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let update = match parse_lcg_body(body, &state_name, &index_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end, update)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[3] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    let exit_with_state = exit_mask.is_some();
    if !exit_with_state {
        return Ok(None);
    }
    if !low_bits_cover_mask(update.low_bit_mask, exit_mask.unwrap_or(u64::MAX)) {
        return Ok(None);
    }

    if loop_end < index_init {
        return Ok(Some(vec![LoweredStmt::Exit(state_init)]));
    }
    let iterations = loop_end - index_init;

    Ok(Some(vec![LoweredStmt::RuntimeLcgLoop {
        iterations,
        state_init,
        mul: update.mul,
        add: update.add,
        exit_with_state: true,
        exit_mask,
    }]))
}

pub(super) fn ensure_u64_type_hint(ty: Option<&TypeName>, span: Span) -> Result<(), Diagnostic> {
    if let Some(TypeName::Int {
        signed: false,
        bits: 64,
    }) = ty
    {
        Ok(())
    } else if ty.is_none() {
        Ok(())
    } else {
        Err(type_error(
            "runtime loop kernel requires u64 variables",
            span,
        ))
    }
}

fn is_runtime_seed_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { name, .. } if name == "runtime_seed")
}

pub(super) fn parse_expr_u64(expr: &Expr) -> Result<u64, Diagnostic> {
    match expr {
        Expr::Number { literal, span } => {
            let value = parse_number_literal(literal, *span)?;
            match value {
                Value::UInt { value, .. } => {
                    u64::try_from(value).map_err(|_| type_error("u64 literal out of range", *span))
                }
                Value::Int { value, .. } => {
                    if value < 0 {
                        return Err(type_error("expected non-negative integer", *span));
                    }
                    u64::try_from(value).map_err(|_| type_error("u64 literal out of range", *span))
                }
                _ => Err(type_error("expected integer literal", *span)),
            }
        }
        _ => Err(type_error("expected numeric literal", expr.span())),
    }
}

fn parse_mut_u64_zero_let(stmt: &Stmt) -> Option<String> {
    let Stmt::Let {
        name,
        mutable,
        ty,
        expr,
        span,
    } = stmt
    else {
        return None;
    };
    if !*mutable {
        return None;
    }
    if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
        return None;
    }
    let init = parse_expr_u64(expr).ok()?;
    if init != 0 {
        return None;
    }
    Some(name.clone())
}

fn parse_mut_zero_u64_array_let(stmt: &Stmt) -> Option<(String, u64)> {
    let Stmt::Let {
        name,
        mutable,
        ty,
        expr,
        ..
    } = stmt
    else {
        return None;
    };
    if !*mutable {
        return None;
    }
    let TypeName::Array { elem, len } = ty.as_ref()? else {
        return None;
    };
    if !matches!(
        elem.as_ref(),
        TypeName::Int {
            signed: false,
            bits: 64
        }
    ) {
        return None;
    }
    let Expr::ArrayLit { elems, .. } = expr else {
        return None;
    };
    if elems.len() as u64 != *len {
        return None;
    }
    if elems
        .iter()
        .any(|elem| parse_expr_u64(elem).ok() != Some(0))
    {
        return None;
    }
    Some((name.clone(), *len))
}

fn parse_mut_u64_array_const_let(stmt: &Stmt, values: &[u64]) -> Option<String> {
    let Stmt::Let {
        name,
        mutable,
        ty,
        expr,
        ..
    } = stmt
    else {
        return None;
    };
    if !*mutable {
        return None;
    }
    let TypeName::Array { elem, len } = ty.as_ref()? else {
        return None;
    };
    if !matches!(
        elem.as_ref(),
        TypeName::Int {
            signed: false,
            bits: 64
        }
    ) || *len != values.len() as u64
    {
        return None;
    }
    let Expr::ArrayLit { elems, .. } = expr else {
        return None;
    };
    if elems.len() != values.len() {
        return None;
    }
    for (elem, expected) in elems.iter().zip(values.iter().copied()) {
        if parse_expr_u64(elem).ok() != Some(expected) {
            return None;
        }
    }
    Some(name.clone())
}

fn parse_mut_u64_array_fill_let(stmt: &Stmt, len: usize, value: u64) -> Option<String> {
    let Stmt::Let {
        name,
        mutable,
        ty,
        expr,
        ..
    } = stmt
    else {
        return None;
    };
    if !*mutable {
        return None;
    }
    let TypeName::Array {
        elem,
        len: array_len,
    } = ty.as_ref()?
    else {
        return None;
    };
    if !matches!(
        elem.as_ref(),
        TypeName::Int {
            signed: false,
            bits: 64
        }
    ) || *array_len != len as u64
    {
        return None;
    }
    let Expr::ArrayLit { elems, .. } = expr else {
        return None;
    };
    if elems.len() != len {
        return None;
    }
    if elems
        .iter()
        .any(|elem| parse_expr_u64(elem).ok() != Some(value))
    {
        return None;
    }
    Some(name.clone())
}

fn parse_const_u64_let(stmt: &Stmt) -> Option<(String, u64)> {
    let Stmt::Let {
        name,
        mutable,
        ty,
        expr,
        span,
    } = stmt
    else {
        return None;
    };
    if *mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
        return None;
    }
    Some((name.clone(), parse_expr_u64(expr).ok()?))
}

fn matches_ident_binop(expr: &Expr, op: BinaryOp, lhs_name: &str, rhs_name: &str) -> bool {
    matches!(
        expr,
        Expr::Binary { op: expr_op, left, right, .. }
            if *expr_op == op
                && matches!(left.as_ref(), Expr::Ident { name, .. } if name == lhs_name)
                && matches!(right.as_ref(), Expr::Ident { name, .. } if name == rhs_name)
    )
}

fn collect_sum_idents<'a>(expr: &'a Expr, out: &mut Vec<&'a str>) -> bool {
    match expr {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
            ..
        } => collect_sum_idents(left, out) && collect_sum_idents(right, out),
        Expr::Ident { name, .. } => {
            out.push(name.as_str());
            true
        }
        _ => false,
    }
}

fn expr_is_sum_of_idents(expr: &Expr, mut names: [&str; 4]) -> bool {
    let mut found = Vec::new();
    if !collect_sum_idents(expr, &mut found) || found.len() != names.len() {
        return false;
    }
    found.sort_unstable();
    names.sort_unstable();
    found == names
}

fn parse_while_upper_bound(cond: &Expr, index_name: &str) -> Result<u64, Diagnostic> {
    match cond {
        Expr::Binary {
            op: BinaryOp::Lt,
            left,
            right,
            ..
        } => match (&**left, &**right) {
            (Expr::Ident { name, .. }, rhs) if name == index_name => parse_expr_u64(rhs),
            _ => Err(type_error(
                "runtime loop kernel expects condition 'i < CONST'",
                cond.span(),
            )),
        },
        _ => Err(type_error(
            "runtime loop kernel expects while condition",
            cond.span(),
        )),
    }
}

fn parse_lcg_body(
    body: &[Stmt],
    state_name: &str,
    index_name: &str,
) -> Result<StateUpdateSpec, Diagnostic> {
    if body.len() != 2 {
        return Err(type_error(
            "runtime loop kernel expects exactly 2 statements in while body",
            body.first()
                .map(|s| stmt_span(s))
                .unwrap_or(Span::new(0, 0)),
        ));
    }

    let mut update = None;
    let mut has_index_inc = false;
    for stmt in body {
        match stmt {
            Stmt::Assign { name, expr, .. } if name == state_name => {
                update = Some(parse_state_update_spec(expr, state_name)?);
            }
            Stmt::Assign { name, expr, .. } if name == index_name => {
                has_index_inc = is_index_increment(expr, index_name)?;
            }
            _ => {
                return Err(type_error(
                    "runtime loop kernel body must only assign state and index",
                    stmt_span(stmt),
                ));
            }
        }
    }

    if !has_index_inc {
        return Err(type_error(
            "runtime loop kernel requires index increment by 1",
            stmt_span(&body[0]),
        ));
    }
    update.ok_or_else(|| {
        type_error(
            "runtime loop kernel requires state update 'state = state * M + A'",
            stmt_span(&body[0]),
        )
    })
}

fn parse_exit_state_mask(expr: &Expr, state_name: &str) -> Option<u64> {
    match expr {
        Expr::Ident { name, .. } if name == state_name => Some(u64::MAX),
        Expr::Binary {
            op: BinaryOp::BitAnd,
            left,
            right,
            ..
        } => match (&**left, &**right) {
            (Expr::Ident { name, .. }, other) if name == state_name => parse_expr_u64(other).ok(),
            (other, Expr::Ident { name, .. }) if name == state_name => parse_expr_u64(other).ok(),
            _ => None,
        },
        _ => None,
    }
}

enum BranchCondSpec {
    PredictableIndexPivot(u64),
    UnpredictableStateThreshold(u64),
}

struct RingWriteLoopSpec {
    update: StateUpdateSpec,
    value_shift: u8,
}

const SORT_WINDOW_STATE_SLOT: usize = 0;
const SORT_WINDOW_INDEX_SLOT: usize = 1;
const SORT_WINDOW_FIRST_SLOT: usize = 2;
const SORT_WINDOW_TEMP_SLOT: usize = 10;

fn parse_seeded_branch_body(
    body: &[Stmt],
    state_name: &str,
    index_name: &str,
) -> Result<SeededBranchKernel, Diagnostic> {
    if body.len() != 2 {
        return Err(type_error(
            "runtime branch kernel expects if/else + index increment in while body",
            body.first()
                .map(|s| stmt_span(s))
                .unwrap_or(Span::new(0, 0)),
        ));
    }

    let (cond, then_branch, else_branch) = match &body[0] {
        Stmt::If {
            cond,
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => (cond, then_branch, else_branch),
        _ => {
            return Err(type_error(
                "runtime branch kernel first statement must be if/else",
                stmt_span(&body[0]),
            ));
        }
    };

    if !is_index_increment(
        match &body[1] {
            Stmt::Assign { name, expr, .. } if name == index_name => expr,
            _ => {
                return Err(type_error(
                    "runtime branch kernel second statement must increment index by 1",
                    stmt_span(&body[1]),
                ));
            }
        },
        index_name,
    )? {
        return Err(type_error(
            "runtime branch kernel requires index increment by 1",
            stmt_span(&body[1]),
        ));
    }

    let then_update = parse_single_state_update_block(then_branch, state_name)?;
    let else_update = parse_single_state_update_block(else_branch, state_name)?;

    match parse_branch_cond_spec(cond, state_name, index_name)? {
        BranchCondSpec::PredictableIndexPivot(pivot) => Ok(SeededBranchKernel::Predictable {
            pivot,
            then_update,
            else_update,
        }),
        BranchCondSpec::UnpredictableStateThreshold(threshold) => {
            Ok(SeededBranchKernel::Unpredictable {
                threshold,
                then_update,
                else_update,
            })
        }
    }
}

fn parse_branch_cond_spec(
    cond: &Expr,
    state_name: &str,
    index_name: &str,
) -> Result<BranchCondSpec, Diagnostic> {
    match cond {
        Expr::Binary {
            op: BinaryOp::Lt,
            left,
            right,
            ..
        } => match (&**left, &**right) {
            (Expr::Ident { name, .. }, rhs) if name == index_name => {
                Ok(BranchCondSpec::PredictableIndexPivot(parse_expr_u64(rhs)?))
            }
            (Expr::Ident { name, .. }, rhs) if name == state_name => Ok(
                BranchCondSpec::UnpredictableStateThreshold(parse_expr_u64(rhs)?),
            ),
            _ => Err(type_error(
                "runtime branch kernel condition must be 'i < CONST' or 'state < CONST'",
                cond.span(),
            )),
        },
        _ => Err(type_error(
            "runtime branch kernel condition must be '<' comparison",
            cond.span(),
        )),
    }
}

fn parse_ring_write_body(
    body: &[Stmt],
    state_name: &str,
    buf_name: &str,
    mask_name: &str,
    index_name: &str,
) -> Result<RingWriteLoopSpec, Diagnostic> {
    if body.len() != 4 {
        return Err(type_error(
            "runtime ring-write kernel expects state update, idx let, store, and index increment",
            body.first().map(stmt_span).unwrap_or(Span::new(0, 0)),
        ));
    }

    let update = match &body[0] {
        Stmt::Assign { name, expr, .. } if name == state_name => {
            parse_state_update_spec(expr, state_name)?
        }
        _ => {
            return Err(type_error(
                "runtime ring-write kernel first statement must assign state",
                stmt_span(&body[0]),
            ));
        }
    };

    let idx_name = match &body[1] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if *mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Err(type_error(
                    "runtime ring-write kernel index binding must be immutable u64",
                    *span,
                ));
            }
            match expr {
                Expr::Binary {
                    op: BinaryOp::BitAnd,
                    left,
                    right,
                    ..
                } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == index_name)
                    && matches!(right.as_ref(), Expr::Ident { name, .. } if name == mask_name) =>
                {
                    name.clone()
                }
                _ => {
                    return Err(type_error(
                        "runtime ring-write kernel index binding must be 'i & mask'",
                        expr.span(),
                    ));
                }
            }
        }
        _ => {
            return Err(type_error(
                "runtime ring-write kernel second statement must bind idx",
                stmt_span(&body[1]),
            ));
        }
    };

    let value_shift = match &body[2] {
        Stmt::AssignIndex {
            name, index, expr, ..
        } if name == buf_name && matches!(index, Expr::Ident { name, .. } if name == &idx_name) => {
            parse_packed_state_expr(expr, state_name).ok_or_else(|| {
                type_error(
                    "runtime ring-write kernel store must write '(state << K) | state'",
                    expr.span(),
                )
            })?
        }
        _ => {
            return Err(type_error(
                "runtime ring-write kernel third statement must store into the ring buffer",
                stmt_span(&body[2]),
            ));
        }
    };

    match &body[3] {
        Stmt::Assign { name, expr, .. }
            if name == index_name && is_index_increment(expr, index_name)? => {}
        _ => {
            return Err(type_error(
                "runtime ring-write kernel must end with index increment",
                stmt_span(&body[3]),
            ));
        }
    }

    Ok(RingWriteLoopSpec {
        update,
        value_shift,
    })
}

fn parse_packed_state_expr(expr: &Expr, state_name: &str) -> Option<u8> {
    match expr {
        Expr::Binary {
            op: BinaryOp::BitOr,
            left,
            right,
            ..
        } => {
            if matches!(right.as_ref(), Expr::Ident { name, .. } if name == state_name) {
                parse_state_shift_expr(left, state_name)
            } else if matches!(left.as_ref(), Expr::Ident { name, .. } if name == state_name) {
                parse_state_shift_expr(right, state_name)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_state_shift_expr(expr: &Expr, state_name: &str) -> Option<u8> {
    match expr {
        Expr::Binary {
            op: BinaryOp::Shl,
            left,
            right,
            ..
        } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == state_name) => {
            let shift = parse_expr_u64(right).ok()?;
            u8::try_from(shift).ok()
        }
        _ => None,
    }
}

fn parse_ring_write_exit(
    expr: &Expr,
    state_name: &str,
    buf_name: &str,
    value_shift: u8,
) -> Option<u64> {
    let Expr::Binary {
        op: BinaryOp::BitAnd,
        left,
        right,
        ..
    } = expr
    else {
        return None;
    };
    let exit_mask = parse_expr_u64(right).ok()?;
    let Expr::Binary {
        op: BinaryOp::BitXor,
        left: xor_left,
        right: xor_right,
        ..
    } = left.as_ref()
    else {
        return None;
    };

    let is_buf0 = |candidate: &Expr| {
        matches!(
            candidate,
            Expr::Index { base, index, .. }
                if matches!(base.as_ref(), Expr::Ident { name, .. } if name == buf_name)
                    && parse_expr_u64(index).ok() == Some(0)
        )
    };

    if parse_packed_state_expr(xor_left, state_name) == Some(value_shift) && is_buf0(xor_right) {
        Some(exit_mask)
    } else if parse_packed_state_expr(xor_right, state_name) == Some(value_shift)
        && is_buf0(xor_left)
    {
        Some(exit_mask)
    } else {
        None
    }
}

fn matches_bloom_filter_build_body(
    body: &[Stmt],
    state_name: &str,
    filter_name: &str,
    index_name: &str,
) -> bool {
    if body.len() != 7 {
        return false;
    }
    matches_lcg_assign(&body[0], state_name)
        && matches_ident_let(&body[1], state_name)
        && matches_lcg_assign(&body[2], state_name)
        && matches_hash_pair_let(&body[3], state_name)
        && matches_lane_init(&body[4])
        && matches_bloom_insert_loop(&body[5], filter_name)
        && matches!(
            &body[6],
            Stmt::Assign { name, expr, .. }
                if name == index_name && is_index_increment(expr, index_name).ok() == Some(true)
        )
}

fn matches_bloom_filter_query_body(
    body: &[Stmt],
    state_name: &str,
    filter_name: &str,
    hits_name: &str,
    index_name: &str,
) -> bool {
    if body.len() != 9 {
        return false;
    }
    matches_lcg_assign(&body[0], state_name)
        && matches_ident_let(&body[1], state_name)
        && matches_lcg_assign(&body[2], state_name)
        && matches_hash_pair_let(&body[3], state_name)
        && matches_hit_init(&body[4])
        && matches_lane_init(&body[5])
        && matches_bloom_check_loop(&body[6], filter_name)
        && matches!(
            &body[7],
            Stmt::Assign { name, expr, .. }
                if name == hits_name && matches!(
                    expr,
                    Expr::Binary {
                        op: BinaryOp::Add,
                        left,
                        right,
                        ..
                    } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == hits_name)
                        && matches!(right.as_ref(), Expr::Ident { name, .. } if name == "hit")
                )
        )
        && matches!(
            &body[8],
            Stmt::Assign { name, expr, .. }
                if name == index_name && is_index_increment(expr, index_name).ok() == Some(true)
        )
}

fn matches_lcg_assign(stmt: &Stmt, state_name: &str) -> bool {
    matches!(
        stmt,
        Stmt::Assign { name, expr, .. }
            if name == state_name && parse_state_update_spec(expr, state_name)
                .map(|spec| spec.mul == 1_664_525 && spec.add == 1_013_904_223 && spec.low_bit_mask == 0xFFFF_FFFF)
                .unwrap_or(false)
    )
}

fn matches_ident_let(stmt: &Stmt, ident_name: &str) -> bool {
    matches!(
        stmt,
        Stmt::Let {
            mutable,
            ty,
            expr,
            span,
            ..
        } if !*mutable
            && ensure_u64_type_hint(ty.as_ref(), *span).is_ok()
            && matches!(expr, Expr::Ident { name, .. } if name == ident_name)
    )
}

fn matches_hash_pair_let(stmt: &Stmt, state_name: &str) -> bool {
    matches!(
        stmt,
        Stmt::Let {
            mutable,
            ty,
            expr,
            span,
            ..
        } if !*mutable
            && ensure_u64_type_hint(ty.as_ref(), *span).is_ok()
            && matches!(
                expr,
                Expr::Binary {
                    op: BinaryOp::BitOr,
                    left,
                    right,
                    ..
                } if matches!(
                    left.as_ref(),
                    Expr::Binary {
                        op: BinaryOp::Shl,
                        right: shift,
                        ..
                    } if parse_expr_u64(shift).ok() == Some(32)
                ) && matches!(right.as_ref(), Expr::Ident { name, .. } if name == state_name)
            )
    )
}

fn matches_lane_init(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } if name == "lane"
            && *mutable
            && ensure_u64_type_hint(ty.as_ref(), *span).is_ok()
            && parse_expr_u64(expr).ok() == Some(0)
    )
}

fn matches_hit_init(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } if name == "hit"
            && *mutable
            && ensure_u64_type_hint(ty.as_ref(), *span).is_ok()
            && parse_expr_u64(expr).ok() == Some(1)
    )
}

fn matches_bloom_insert_loop(stmt: &Stmt, filter_name: &str) -> bool {
    let Stmt::While { cond, body, .. } = stmt else {
        return false;
    };
    if parse_while_upper_bound(cond, "lane").ok() != Some(4) || body.len() != 4 {
        return false;
    }
    matches_bloom_word_let(&body[0])
        && matches_bloom_bit_let(&body[1])
        && matches_bloom_insert_store(&body[2], filter_name)
        && matches!(
            &body[3],
            Stmt::Assign { name, expr, .. }
                if name == "lane" && is_index_increment(expr, "lane").ok() == Some(true)
        )
}

fn matches_bloom_check_loop(stmt: &Stmt, filter_name: &str) -> bool {
    let Stmt::While { cond, body, .. } = stmt else {
        return false;
    };
    if parse_while_upper_bound(cond, "lane").ok() != Some(4) || body.len() != 4 {
        return false;
    }
    matches_bloom_word_let(&body[0])
        && matches_bloom_bit_let(&body[1])
        && matches_bloom_hit_update(&body[2], filter_name)
        && matches!(
            &body[3],
            Stmt::Assign { name, expr, .. }
                if name == "lane" && is_index_increment(expr, "lane").ok() == Some(true)
        )
}

fn matches_bloom_word_let(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } if name == "word"
            && !*mutable
            && ensure_u64_type_hint(ty.as_ref(), *span).is_ok()
            && matches_bloom_word_expr(expr)
    )
}

fn matches_bloom_bit_let(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } if name == "bit"
            && !*mutable
            && ensure_u64_type_hint(ty.as_ref(), *span).is_ok()
            && matches_bloom_bit_expr(expr)
    )
}

fn matches_bloom_word_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Binary {
            op: BinaryOp::BitAnd,
            left,
            right,
            ..
        } if parse_expr_u64(right).ok() == Some(255)
            && matches!(
                left.as_ref(),
                Expr::Binary {
                    op: BinaryOp::Shr,
                    left: hash,
                    right: shift,
                    ..
                } if matches!(hash.as_ref(), Expr::Ident { name, .. } if name == "h")
                    && matches!(
                        shift.as_ref(),
                        Expr::Binary {
                            op: BinaryOp::Mul,
                            left,
                            right,
                            ..
                        } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == "lane")
                            && parse_expr_u64(right).ok() == Some(8)
                    )
            )
    )
}

fn matches_bloom_bit_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Binary {
            op: BinaryOp::BitAnd,
            left,
            right,
            ..
        } if parse_expr_u64(right).ok() == Some(63)
            && matches!(
                left.as_ref(),
                Expr::Binary {
                    op: BinaryOp::Shr,
                    left: hash,
                    right: shift,
                    ..
                } if matches!(hash.as_ref(), Expr::Ident { name, .. } if name == "h")
                    && matches!(
                        shift.as_ref(),
                        Expr::Binary {
                            op: BinaryOp::Add,
                            left,
                            right,
                            ..
                        } if parse_expr_u64(right).ok() == Some(3)
                            && matches!(
                                left.as_ref(),
                                Expr::Binary {
                                    op: BinaryOp::Mul,
                                    left,
                                    right,
                                    ..
                                } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == "lane")
                                    && parse_expr_u64(right).ok() == Some(11)
                            )
                    )
            )
    )
}

fn matches_bloom_insert_store(stmt: &Stmt, filter_name: &str) -> bool {
    matches!(
        stmt,
        Stmt::AssignIndex {
            name,
            index,
            expr,
            ..
        } if name == filter_name
            && matches!(index, Expr::Ident { name, .. } if name == "word")
            && matches!(
                expr,
                Expr::Binary {
                    op: BinaryOp::BitOr,
                    left,
                    right,
                    ..
                } if matches!(
                    left.as_ref(),
                    Expr::Index { base, index, .. }
                        if matches!(base.as_ref(), Expr::Ident { name, .. } if name == filter_name)
                            && matches!(index.as_ref(), Expr::Ident { name, .. } if name == "word")
                ) && matches!(
                    right.as_ref(),
                    Expr::Binary {
                        op: BinaryOp::Shl,
                        left,
                        right,
                        ..
                    } if parse_expr_u64(left).ok() == Some(1)
                        && matches!(right.as_ref(), Expr::Ident { name, .. } if name == "bit")
                )
            )
    )
}

fn matches_bloom_hit_update(stmt: &Stmt, filter_name: &str) -> bool {
    matches!(
        stmt,
        Stmt::Assign { name, expr, .. }
            if name == "hit" && matches!(
                expr,
                Expr::Binary {
                    op: BinaryOp::BitAnd,
                    left,
                    right,
                    ..
                } if matches!(left.as_ref(), Expr::Ident { name, .. } if name == "hit")
                    && matches!(
                        right.as_ref(),
                        Expr::Binary {
                            op: BinaryOp::BitAnd,
                            left,
                            right,
                            ..
                        } if parse_expr_u64(right).ok() == Some(1)
                            && matches!(
                                left.as_ref(),
                                Expr::Binary {
                                    op: BinaryOp::Shr,
                                    left,
                                    right,
                                    ..
                                } if matches!(
                                    left.as_ref(),
                                    Expr::Index { base, index, .. }
                                        if matches!(base.as_ref(), Expr::Ident { name, .. } if name == filter_name)
                                            && matches!(index.as_ref(), Expr::Ident { name, .. } if name == "word")
                                ) && matches!(right.as_ref(), Expr::Ident { name, .. } if name == "bit")
                            )
                    )
            )
    )
}

fn build_runtime_sort_window_program(
    body: &[Stmt],
    state_name: &str,
    state_init: u64,
    index_name: &str,
    index_init: u64,
    window_name: &str,
    loop_end: u64,
) -> Option<RuntimeProgram> {
    if body.len() != 13 {
        return None;
    }

    let update = match &body[0] {
        Stmt::Assign { name, expr, .. } if name == state_name => {
            parse_state_update_spec(expr, state_name).ok()?
        }
        _ => return None,
    };
    if update.low_bit_mask != 0xFFFF_FFFF {
        return None;
    }

    for (offset, index) in (0u64..8).enumerate() {
        let stmt = &body[1 + offset];
        let expected = sort_window_formula(index)?;
        if !matches_sort_window_assign(stmt, window_name, index, &expected) {
            return None;
        }
    }

    if !matches_sort_window_bubble_sort(&body[9], &body[10], window_name) {
        return None;
    }

    match &body[11] {
        Stmt::Assign { name, expr, .. } if name == state_name => {
            if !matches_sort_window_state_mix(expr, state_name, window_name) {
                return None;
            }
        }
        _ => return None,
    }

    match &body[12] {
        Stmt::Assign { name, expr, .. }
            if name == index_name && is_index_increment(expr, index_name).ok()? => {}
        _ => return None,
    }

    let mut instrs = Vec::new();
    instrs.push(RuntimeInstr::Mov {
        dst: SORT_WINDOW_STATE_SLOT,
        src: RuntimeOperand::Imm(state_init),
    });
    instrs.push(RuntimeInstr::Mov {
        dst: SORT_WINDOW_INDEX_SLOT,
        src: RuntimeOperand::Imm(index_init),
    });

    let loop_header = instrs.len();
    instrs.push(RuntimeInstr::JumpIfCmpFalse {
        op: RuntimeCmpOp::LtUnsigned,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_INDEX_SLOT),
        rhs: RuntimeOperand::Imm(loop_end),
        target: usize::MAX,
    });

    instrs.push(RuntimeInstr::BinOp {
        dst: SORT_WINDOW_TEMP_SLOT,
        op: RuntimeBinOp::Mul,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(u64::from(update.mul)),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: SORT_WINDOW_TEMP_SLOT,
        op: RuntimeBinOp::Add,
        rhs: RuntimeOperand::Imm(u64::from(update.add)),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: SORT_WINDOW_TEMP_SLOT,
        op: RuntimeBinOp::BitAnd,
        rhs: RuntimeOperand::Imm(update.low_bit_mask),
    });
    instrs.push(RuntimeInstr::Mov {
        dst: SORT_WINDOW_STATE_SLOT,
        src: RuntimeOperand::Slot(SORT_WINDOW_TEMP_SLOT),
    });

    instrs.push(RuntimeInstr::Mov {
        dst: window_slot(0),
        src: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(1),
        op: RuntimeBinOp::BitXor,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(2_779_096_485),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(2),
        op: RuntimeBinOp::Add,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Slot(SORT_WINDOW_INDEX_SLOT),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: window_slot(2),
        op: RuntimeBinOp::BitAnd,
        rhs: RuntimeOperand::Imm(0xFFFF_FFFF),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(3),
        op: RuntimeBinOp::Mul,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(3),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(4),
        op: RuntimeBinOp::Sub,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Slot(SORT_WINDOW_INDEX_SLOT),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: window_slot(4),
        op: RuntimeBinOp::BitAnd,
        rhs: RuntimeOperand::Imm(0xFFFF_FFFF),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(5),
        op: RuntimeBinOp::ShrUnsigned,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(3),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(6),
        op: RuntimeBinOp::Shl,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(1),
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: window_slot(7),
        op: RuntimeBinOp::Add,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(7),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: window_slot(7),
        op: RuntimeBinOp::BitAnd,
        rhs: RuntimeOperand::Imm(0xFFFF_FFFF),
    });

    for (left, right) in runtime_fixed_sort_pairs(8)? {
        instrs.push(RuntimeInstr::CompareSwap {
            left: window_slot(left),
            right: window_slot(right),
            signed: false,
        });
    }

    instrs.push(RuntimeInstr::BinOp {
        dst: SORT_WINDOW_TEMP_SLOT,
        op: RuntimeBinOp::BitXor,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Slot(window_slot(0)),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: SORT_WINDOW_TEMP_SLOT,
        op: RuntimeBinOp::BitXor,
        rhs: RuntimeOperand::Slot(window_slot(7)),
    });
    instrs.push(RuntimeInstr::Mov {
        dst: SORT_WINDOW_STATE_SLOT,
        src: RuntimeOperand::Slot(SORT_WINDOW_TEMP_SLOT),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: SORT_WINDOW_INDEX_SLOT,
        op: RuntimeBinOp::Add,
        rhs: RuntimeOperand::Imm(1),
    });
    instrs.push(RuntimeInstr::Jump {
        target: loop_header,
    });

    let exit_target = instrs.len();
    instrs[loop_header].clone_from(&RuntimeInstr::JumpIfCmpFalse {
        op: RuntimeCmpOp::LtUnsigned,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_INDEX_SLOT),
        rhs: RuntimeOperand::Imm(loop_end),
        target: exit_target,
    });
    instrs.push(RuntimeInstr::BinOp {
        dst: SORT_WINDOW_TEMP_SLOT,
        op: RuntimeBinOp::BitAnd,
        lhs: RuntimeOperand::Slot(SORT_WINDOW_STATE_SLOT),
        rhs: RuntimeOperand::Imm(127),
    });
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Slot(SORT_WINDOW_TEMP_SLOT),
    });

    Some(RuntimeProgram { slots: 11, instrs })
}

#[derive(Clone, Copy)]
enum SortWindowFormula {
    State,
    XorConst(u64),
    AddIndexMasked(u64),
    MulConst(u64),
    SubIndexMasked(u64),
    Shr(u8),
    Shl(u8),
    AddConstMasked(u64, u64),
}

fn sort_window_formula(index: u64) -> Option<SortWindowFormula> {
    match index {
        0 => Some(SortWindowFormula::State),
        1 => Some(SortWindowFormula::XorConst(2_779_096_485)),
        2 => Some(SortWindowFormula::AddIndexMasked(0xFFFF_FFFF)),
        3 => Some(SortWindowFormula::MulConst(3)),
        4 => Some(SortWindowFormula::SubIndexMasked(0xFFFF_FFFF)),
        5 => Some(SortWindowFormula::Shr(3)),
        6 => Some(SortWindowFormula::Shl(1)),
        7 => Some(SortWindowFormula::AddConstMasked(7, 0xFFFF_FFFF)),
        _ => None,
    }
}

fn matches_sort_window_assign(
    stmt: &Stmt,
    window_name: &str,
    index: u64,
    formula: &SortWindowFormula,
) -> bool {
    let Stmt::AssignIndex {
        name,
        index: idx_expr,
        expr,
        ..
    } = stmt
    else {
        return false;
    };
    if name != window_name || parse_expr_u64(idx_expr).ok() != Some(index) {
        return false;
    }
    matches_sort_window_formula_expr(expr, formula, "state", "i")
}

fn matches_sort_window_formula_expr(
    expr: &Expr,
    formula: &SortWindowFormula,
    state_name: &str,
    index_name: &str,
) -> bool {
    match formula {
        SortWindowFormula::State => matches!(expr, Expr::Ident { name, .. } if name == state_name),
        SortWindowFormula::XorConst(value) => {
            matches_ident_const_binop(expr, BinaryOp::BitXor, state_name, *value)
        }
        SortWindowFormula::AddIndexMasked(mask) => {
            matches_masked_index_binop(expr, BinaryOp::Add, state_name, index_name, *mask)
        }
        SortWindowFormula::MulConst(value) => {
            matches_ident_const_binop(expr, BinaryOp::Mul, state_name, *value)
        }
        SortWindowFormula::SubIndexMasked(mask) => {
            matches_masked_index_binop(expr, BinaryOp::Sub, state_name, index_name, *mask)
        }
        SortWindowFormula::Shr(shift) => {
            matches_ident_const_binop(expr, BinaryOp::Shr, state_name, u64::from(*shift))
        }
        SortWindowFormula::Shl(shift) => {
            matches_ident_const_binop(expr, BinaryOp::Shl, state_name, u64::from(*shift))
        }
        SortWindowFormula::AddConstMasked(value, mask) => {
            matches_masked_const_binop(expr, BinaryOp::Add, state_name, *value, *mask)
        }
    }
}

fn matches_ident_const_binop(expr: &Expr, op: BinaryOp, lhs_name: &str, rhs_const: u64) -> bool {
    matches!(
        expr,
        Expr::Binary { op: expr_op, left, right, .. }
            if *expr_op == op
                && matches!(left.as_ref(), Expr::Ident { name, .. } if name == lhs_name)
                && parse_expr_u64(right).ok() == Some(rhs_const)
    )
}

fn matches_masked_index_binop(
    expr: &Expr,
    inner_op: BinaryOp,
    state_name: &str,
    index_name: &str,
    mask: u64,
) -> bool {
    let Expr::Binary {
        op: BinaryOp::BitAnd,
        left,
        right,
        ..
    } = expr
    else {
        return false;
    };
    if parse_expr_u64(right).ok() != Some(mask) {
        return false;
    }
    matches!(
        left.as_ref(),
        Expr::Binary { op, left: inner_left, right: inner_right, .. }
            if *op == inner_op
                && matches!(inner_left.as_ref(), Expr::Ident { name, .. } if name == state_name)
                && matches!(inner_right.as_ref(), Expr::Ident { name, .. } if name == index_name)
    )
}

fn matches_masked_const_binop(
    expr: &Expr,
    inner_op: BinaryOp,
    state_name: &str,
    rhs_const: u64,
    mask: u64,
) -> bool {
    let Expr::Binary {
        op: BinaryOp::BitAnd,
        left,
        right,
        ..
    } = expr
    else {
        return false;
    };
    if parse_expr_u64(right).ok() != Some(mask) {
        return false;
    }
    matches_ident_const_binop(left, inner_op, state_name, rhs_const)
}

fn matches_sort_window_bubble_sort(pass_stmt: &Stmt, sort_stmt: &Stmt, window_name: &str) -> bool {
    let Stmt::Let {
        name,
        mutable,
        ty,
        expr,
        span,
    } = pass_stmt
    else {
        return false;
    };
    if name != "pass" || !*mutable || ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
        return false;
    }
    if parse_expr_u64(expr).ok() != Some(0) {
        return false;
    }
    let pass_name = name.as_str();

    let Stmt::While { cond, body, .. } = sort_stmt else {
        return false;
    };
    if !matches!(
        cond,
        Expr::Binary { op: BinaryOp::Lt, left, right, .. }
            if matches!(left.as_ref(), Expr::Ident { name, .. } if name == pass_name)
                && parse_expr_u64(right).ok() == Some(8)
    ) {
        return false;
    }
    if body.len() != 3 {
        return false;
    }

    let Stmt::Let {
        name: j_name,
        mutable: j_mut,
        ty: j_ty,
        expr: j_init,
        span: j_span,
    } = &body[0]
    else {
        return false;
    };
    if !*j_mut
        || ensure_u64_type_hint(j_ty.as_ref(), *j_span).is_err()
        || parse_expr_u64(j_init).ok() != Some(0)
    {
        return false;
    }

    let Stmt::While {
        cond: inner_cond,
        body: inner_body,
        ..
    } = &body[1]
    else {
        return false;
    };
    if !matches_sort_window_inner_bubble_cond(inner_cond, j_name, pass_name) {
        return false;
    }
    if inner_body.len() != 2 {
        return false;
    }
    if !matches_sort_window_inner_swap(&inner_body[0], window_name, j_name) {
        return false;
    }
    match &inner_body[1] {
        Stmt::Assign { name, expr, .. }
            if name == j_name && is_index_increment(expr, j_name).ok() == Some(true) => {}
        _ => return false,
    }

    match &body[2] {
        Stmt::Assign { name, expr, .. }
            if name == pass_name && is_index_increment(expr, pass_name).ok() == Some(true) => {}
        _ => return false,
    }
    true
}

fn matches_sort_window_inner_bubble_cond(expr: &Expr, j_name: &str, pass_name: &str) -> bool {
    matches!(
        expr,
        Expr::Binary { op: BinaryOp::Lt, left, right, .. }
            if matches!(
                left.as_ref(),
                Expr::Binary { op: BinaryOp::Add, left: add_left, right: add_right, .. }
                    if matches!(add_left.as_ref(), Expr::Ident { name, .. } if name == j_name)
                        && parse_expr_u64(add_right).ok() == Some(1)
            )
                && matches!(
                    right.as_ref(),
                    Expr::Binary { op: BinaryOp::Sub, left: sub_left, right: sub_right, .. }
                        if parse_expr_u64(sub_left).ok() == Some(8)
                            && matches!(sub_right.as_ref(), Expr::Ident { name, .. } if name == pass_name)
                )
    )
}

fn matches_sort_window_inner_swap(stmt: &Stmt, window_name: &str, j_name: &str) -> bool {
    let Stmt::If {
        cond,
        then_branch,
        else_branch,
        ..
    } = stmt
    else {
        return false;
    };
    if else_branch.is_some() || then_branch.len() != 3 {
        return false;
    }
    if !matches_sort_window_swap_cond(cond, window_name, j_name) {
        return false;
    }

    let Stmt::Let {
        name: tmp_name,
        mutable: false,
        ty,
        expr,
        span,
    } = &then_branch[0]
    else {
        return false;
    };
    if ensure_u64_type_hint(ty.as_ref(), *span).is_err()
        || !matches_window_index_expr(expr, window_name, j_name, 0)
    {
        return false;
    }

    let first_store_ok = matches!(
        &then_branch[1],
        Stmt::AssignIndex { name, index, expr, .. }
            if name == window_name
                && matches!(index, Expr::Ident { name, .. } if name == j_name)
                && matches_window_index_expr(expr, window_name, j_name, 1)
    );
    let second_store_ok = matches!(
        &then_branch[2],
        Stmt::AssignIndex { name, index, expr, .. }
            if name == window_name
                && matches_window_index_add_one(index, j_name)
                && matches!(expr, Expr::Ident { name, .. } if name == tmp_name)
    );
    first_store_ok && second_store_ok
}

fn matches_sort_window_swap_cond(expr: &Expr, window_name: &str, j_name: &str) -> bool {
    matches!(
        expr,
        Expr::Binary { op: BinaryOp::Gt, left, right, .. }
            if matches_window_index_expr(left, window_name, j_name, 0)
                && matches_window_index_expr(right, window_name, j_name, 1)
    )
}

fn matches_window_index_expr(expr: &Expr, window_name: &str, index_name: &str, delta: u64) -> bool {
    matches!(
        expr,
        Expr::Index { base, index, .. }
            if matches!(base.as_ref(), Expr::Ident { name, .. } if name == window_name)
                && if delta == 0 {
                    matches!(index.as_ref(), Expr::Ident { name, .. } if name == index_name)
                } else {
                    matches_window_index_add_one(index, index_name)
                }
    )
}

fn matches_window_index_add_one(expr: &Expr, index_name: &str) -> bool {
    matches!(
        expr,
        Expr::Binary { op: BinaryOp::Add, left, right, .. }
            if matches!(left.as_ref(), Expr::Ident { name, .. } if name == index_name)
                && parse_expr_u64(right).ok() == Some(1)
    )
}

fn window_slot(index: usize) -> usize {
    SORT_WINDOW_FIRST_SLOT + index
}

fn matches_sort_window_state_mix(expr: &Expr, state_name: &str, window_name: &str) -> bool {
    let Expr::Binary {
        op: BinaryOp::BitXor,
        left,
        right,
        ..
    } = expr
    else {
        return false;
    };
    let left_ok = matches!(
        left.as_ref(),
        Expr::Binary { op: BinaryOp::BitXor, left: inner_left, right: inner_right, .. }
            if matches!(inner_left.as_ref(), Expr::Ident { name, .. } if name == state_name)
                && matches!(
                    inner_right.as_ref(),
                    Expr::Index { base, index, .. }
                        if matches!(base.as_ref(), Expr::Ident { name, .. } if name == window_name)
                            && parse_expr_u64(index).ok() == Some(0)
                )
    );
    let right_ok = matches!(
        right.as_ref(),
        Expr::Index { base, index, .. }
            if matches!(base.as_ref(), Expr::Ident { name, .. } if name == window_name)
                && parse_expr_u64(index).ok() == Some(7)
    );
    left_ok && right_ok
}

fn parse_single_state_update_block(
    stmts: &[Stmt],
    state_name: &str,
) -> Result<StateUpdateSpec, Diagnostic> {
    if stmts.len() != 1 {
        return Err(type_error(
            "runtime branch kernel then/else blocks must have one state assignment",
            stmts
                .first()
                .map(|s| stmt_span(s))
                .unwrap_or(Span::new(0, 0)),
        ));
    }
    match &stmts[0] {
        Stmt::Assign { name, expr, .. } if name == state_name => {
            parse_state_update_spec(expr, state_name)
        }
        _ => Err(type_error(
            "runtime branch kernel then/else blocks must assign state",
            stmt_span(&stmts[0]),
        )),
    }
}

fn parse_lcg_for_body(body: &[Stmt], state_name: &str) -> Result<StateUpdateSpec, Diagnostic> {
    if body.len() != 1 {
        return Err(type_error(
            "runtime for kernel expects exactly 1 statement in loop body",
            body.first()
                .map(|s| stmt_span(s))
                .unwrap_or(Span::new(0, 0)),
        ));
    }

    match &body[0] {
        Stmt::Assign { name, expr, .. } if name == state_name => {
            parse_state_update_spec(expr, state_name)
        }
        _ => Err(type_error(
            "runtime for kernel body must only assign state",
            stmt_span(&body[0]),
        )),
    }
}

fn parse_state_update(expr: &Expr, state_name: &str) -> Result<(u32, u32), Diagnostic> {
    let update = parse_state_update_spec(expr, state_name)?;
    Ok((update.mul, update.add))
}

fn parse_state_update_spec(expr: &Expr, state_name: &str) -> Result<StateUpdateSpec, Diagnostic> {
    let (expr, low_bit_mask) = strip_low_bit_mask(expr, "state update")?;
    match expr {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
            ..
        } => match (&**left, &**right) {
            (Expr::Ident { name, .. }, rhs) if name == state_name => {
                let add = parse_expr_u64(rhs)?;
                let add = u32::try_from(add)
                    .map_err(|_| type_error("add constant must fit u32", expr.span()))?;
                Ok(StateUpdateSpec {
                    mul: 1,
                    add,
                    low_bit_mask,
                })
            }
            _ => {
                let add = parse_expr_u64(right)?;
                match &**left {
                    Expr::Binary {
                        op: BinaryOp::Mul,
                        left: mul_left,
                        right: mul_right,
                        ..
                    } => match &**mul_left {
                        Expr::Ident { name, .. } if name == state_name => {
                            let mul = parse_expr_u64(mul_right)?;
                            let mul = u32::try_from(mul).map_err(|_| {
                                type_error("mul constant must fit u32", expr.span())
                            })?;
                            let add = u32::try_from(add).map_err(|_| {
                                type_error("add constant must fit u32", expr.span())
                            })?;
                            Ok(StateUpdateSpec {
                                mul,
                                add,
                                low_bit_mask,
                            })
                        }
                        _ => Err(type_error(
                            "state update must multiply the state variable",
                            expr.span(),
                        )),
                    },
                    _ => Err(type_error(
                        "state update must be 'state * M + A' or 'state + A'",
                        expr.span(),
                    )),
                }
            }
        },
        Expr::Binary {
            op: BinaryOp::Mul,
            left: mul_left,
            right: mul_right,
            ..
        } => match &**mul_left {
            Expr::Ident { name, .. } if name == state_name => {
                let mul = parse_expr_u64(mul_right)?;
                let mul = u32::try_from(mul)
                    .map_err(|_| type_error("mul constant must fit u32", expr.span()))?;
                Ok(StateUpdateSpec {
                    mul,
                    add: 0,
                    low_bit_mask,
                })
            }
            _ => Err(type_error(
                "state update must multiply the state variable",
                expr.span(),
            )),
        },
        _ => Err(type_error(
            "state update must be 'state * M + A' or 'state + A'",
            expr.span(),
        )),
    }
}

pub(super) fn is_index_increment(expr: &Expr, index_name: &str) -> Result<bool, Diagnostic> {
    match expr {
        Expr::Binary {
            op: BinaryOp::Add,
            left,
            right,
            ..
        } => match (&**left, &**right) {
            (Expr::Ident { name, .. }, rhs) if name == index_name => Ok(parse_expr_u64(rhs)? == 1),
            _ => Ok(false),
        },
        _ => Ok(false),
    }
}

pub(super) fn try_lower_runtime_for_kernel(
    main: &Function,
) -> Result<Option<Vec<LoweredStmt>>, Diagnostic> {
    if main.body.len() != 3 {
        return Ok(None);
    }

    let (state_name, state_init) = match &main.body[0] {
        Stmt::Let {
            name,
            mutable,
            ty,
            expr,
            span,
        } => {
            if !*mutable {
                return Ok(None);
            }
            if ensure_u64_type_hint(ty.as_ref(), *span).is_err() {
                return Ok(None);
            }
            let init = match parse_expr_u64(expr) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (name.clone(), init)
        }
        _ => return Ok(None),
    };

    let (iterations, update) = match &main.body[1] {
        Stmt::For {
            start, end, body, ..
        } => {
            let start = match parse_expr_u64(start) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let end = match parse_expr_u64(end) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            let update = match parse_lcg_for_body(body, &state_name) {
                Ok(v) => v,
                Err(_) => return Ok(None),
            };
            (end.saturating_sub(start), update)
        }
        _ => return Ok(None),
    };

    let exit_mask = match &main.body[2] {
        Stmt::Exit { expr, .. } => parse_exit_state_mask(expr, &state_name),
        _ => None,
    };
    let exit_with_state = exit_mask.is_some();
    if !exit_with_state {
        return Ok(None);
    }
    if !low_bits_cover_mask(update.low_bit_mask, exit_mask.unwrap_or(u64::MAX)) {
        return Ok(None);
    }

    if iterations == 0 {
        return Ok(Some(vec![LoweredStmt::Exit(state_init)]));
    }

    Ok(Some(vec![LoweredStmt::RuntimeLcgLoop {
        iterations,
        state_init,
        mul: update.mul,
        add: update.add,
        exit_with_state: true,
        exit_mask,
    }]))
}
