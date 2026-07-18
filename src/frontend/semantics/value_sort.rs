//! Deterministic semantic sorting plans and implementations.

use super::*;

pub(super) fn should_use_runtime_radix_kernel(len: usize, bits: u16) -> bool {
    if !matches!(bits, 32 | 64) {
        return false;
    }
    // Small fixed arrays are better served by sorting networks in this backend.
    len >= 32
}

pub(super) fn runtime_fixed_sort_pairs(len: usize) -> Option<Vec<(usize, usize)>> {
    if len <= 1 {
        return Some(Vec::new());
    }
    match len {
        8 => return Some(SORT_NETWORK_8.to_vec()),
        9 => return Some(SORT_NETWORK_9.to_vec()),
        10 => return Some(SORT_NETWORK_10.to_vec()),
        11 => return Some(SORT_NETWORK_11.to_vec()),
        12 => return Some(SORT_NETWORK_12.to_vec()),
        13 => return Some(SORT_NETWORK_13.to_vec()),
        14 => return Some(SORT_NETWORK_14.to_vec()),
        15 => return Some(SORT_NETWORK_15.to_vec()),
        16 => return Some(SORT_NETWORK_16.to_vec()),
        32 => return Some(runtime_oddeven_merge_power2_pairs(32)),
        64 => return Some(runtime_oddeven_merge_power2_pairs(64)),
        _ => {}
    }
    if len.is_power_of_two() && len <= 512 {
        return Some(runtime_oddeven_merge_power2_pairs(len));
    }
    if len <= 128 {
        return Some(runtime_odd_even_pairs(len));
    }
    None
}

fn runtime_odd_even_pairs(len: usize) -> Vec<(usize, usize)> {
    let mut pairs = Vec::with_capacity(len * len / 2);
    for pass in 0..len {
        let mut j = pass % 2;
        while j + 1 < len {
            pairs.push((j, j + 1));
            j += 2;
        }
    }
    pairs
}

fn runtime_oddeven_merge_power2_pairs(len: usize) -> Vec<(usize, usize)> {
    debug_assert!(len.is_power_of_two());
    let mut pairs = Vec::with_capacity(len * len / 2);
    runtime_oddeven_merge_sort_rec(0, len, &mut pairs);
    pairs
}

fn runtime_oddeven_merge_sort_rec(lo: usize, n: usize, pairs: &mut Vec<(usize, usize)>) {
    if n <= 1 {
        return;
    }
    let m = n / 2;
    runtime_oddeven_merge_sort_rec(lo, m, pairs);
    runtime_oddeven_merge_sort_rec(lo + m, m, pairs);
    runtime_oddeven_merge_rec(lo, n, 1, pairs);
}

fn runtime_oddeven_merge_rec(lo: usize, n: usize, r: usize, pairs: &mut Vec<(usize, usize)>) {
    let step = r * 2;
    if step < n {
        runtime_oddeven_merge_rec(lo, n, step, pairs);
        runtime_oddeven_merge_rec(lo + r, n, step, pairs);
        let mut i = lo + r;
        while i + r < lo + n {
            pairs.push((i, i + r));
            i += step;
        }
    } else {
        pairs.push((lo, lo + r));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortStability {
    Stable,
    Unstable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortAlgorithm {
    Auto,
    RadixOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SortPlan {
    stability: SortStability,
    algorithm: SortAlgorithm,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CachedSortKey {
    Bool(bool),
    Str(String),
    Int { bits: u16, value: i128 },
    UInt { bits: u16, value: u128 },
    Float { bits: u16, ordered: u64 },
    Enum { name: String, variant: String },
    Struct(Vec<(String, CachedSortKey)>),
}

impl SortPlan {
    pub(super) fn auto_unstable() -> Self {
        Self {
            stability: SortStability::Unstable,
            algorithm: SortAlgorithm::Auto,
        }
    }

    pub(super) fn auto_stable() -> Self {
        Self {
            stability: SortStability::Stable,
            algorithm: SortAlgorithm::Auto,
        }
    }

    pub(super) fn radix_unstable() -> Self {
        Self {
            stability: SortStability::Unstable,
            algorithm: SortAlgorithm::RadixOnly,
        }
    }

    pub(super) fn radix_stable() -> Self {
        Self {
            stability: SortStability::Stable,
            algorithm: SortAlgorithm::RadixOnly,
        }
    }
}

fn order_values_for_sort(
    left: &Value,
    right: &Value,
    compare_fn: Option<&str>,
    span: Span,
    index: &ProgramIndex,
    env_snapshot: &Env,
) -> Result<std::cmp::Ordering, Diagnostic> {
    if let Some(compare_name) = compare_fn {
        let left_before_right =
            call_sort_compare_function(compare_name, left, right, index, env_snapshot, span)?;
        let right_before_left =
            call_sort_compare_function(compare_name, right, left, index, env_snapshot, span)?;
        if left_before_right && right_before_left {
            return Err(type_error(
                format!(
                    "comparator '{}' is inconsistent: both a<b and b<a are true",
                    compare_name
                )
                .as_str(),
                span,
            ));
        }
        return Ok(if left_before_right {
            std::cmp::Ordering::Less
        } else if right_before_left {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        });
    }

    match (left, right) {
        (Value::Int { value: av, .. }, Value::Int { value: bv, .. }) => Ok(av.cmp(bv)),
        (Value::UInt { value: av, .. }, Value::UInt { value: bv, .. }) => Ok(av.cmp(bv)),
        (Value::Str(av), Value::Str(bv)) => Ok(av.cmp(bv)),
        (Value::Bool(av), Value::Bool(bv)) => Ok(av.cmp(bv)),
        (Value::Enum { .. }, Value::Enum { .. }) => value_partial_cmp(left, right, span),
        (Value::Float { value: av, .. }, Value::Float { value: bv, .. }) => av
            .partial_cmp(bv)
            .ok_or_else(|| type_error("float comparison with NaN is not supported", span)),
        _ => value_partial_cmp(left, right, span),
    }
}

fn build_cached_sort_key(value: &Value, span: Span) -> Result<Option<CachedSortKey>, Diagnostic> {
    match value {
        Value::Bool(v) => Ok(Some(CachedSortKey::Bool(*v))),
        Value::Str(text) => Ok(Some(CachedSortKey::Str(text.clone()))),
        Value::Int { bits, value } => Ok(Some(CachedSortKey::Int {
            bits: *bits,
            value: *value,
        })),
        Value::UInt { bits, value } => Ok(Some(CachedSortKey::UInt {
            bits: *bits,
            value: *value,
        })),
        Value::Float { bits, value } => {
            if !value.is_finite() {
                return Err(type_error(
                    "float comparison with NaN is not supported",
                    span,
                ));
            }
            let bits64 = value.to_bits();
            let ordered = if (bits64 >> 63) != 0 {
                !bits64
            } else {
                bits64 ^ 0x8000_0000_0000_0000u64
            };
            Ok(Some(CachedSortKey::Float {
                bits: *bits,
                ordered,
            }))
        }
        Value::Enum {
            name,
            variant,
            payload: EnumPayloadValue::Unit,
            ..
        } => Ok(Some(CachedSortKey::Enum {
            name: name.clone(),
            variant: variant.clone(),
        })),
        Value::Enum { .. } => Ok(None),
        Value::Struct { fields, .. } => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            let mut out = Vec::with_capacity(keys.len());
            for key in keys {
                let Some(field_key) =
                    build_cached_sort_key(fields.get(key).expect("field should exist"), span)?
                else {
                    return Ok(None);
                };
                out.push((key.clone(), field_key));
            }
            Ok(Some(CachedSortKey::Struct(out)))
        }
        Value::Char(_)
        | Value::Ref { .. }
        | Value::Array { .. }
        | Value::List { .. }
        | Value::Dict { .. }
        | Value::Map { .. } => Ok(None),
    }
}

fn try_sort_with_cached_keys(
    elems: &mut Vec<Value>,
    compare_fn: Option<&str>,
    span: Span,
    index: &ProgramIndex,
    env_snapshot: &Env,
    plan: SortPlan,
) -> Result<bool, Diagnostic> {
    if compare_fn.is_some() || elems.len() < 16 {
        return Ok(false);
    }
    let mut decorated: Vec<(CachedSortKey, Value)> = Vec::with_capacity(elems.len());
    let mut iter = std::mem::take(elems).into_iter();
    while let Some(value) = iter.next() {
        let Some(key) = build_cached_sort_key(&value, span)? else {
            elems.extend(decorated.into_iter().map(|(_, v)| v));
            elems.push(value);
            elems.extend(iter);
            return Ok(false);
        };
        decorated.push((key, value));
    }

    let mut err: Option<Diagnostic> = None;
    let mut key_cmp =
        |a: &(CachedSortKey, Value), b: &(CachedSortKey, Value)| -> std::cmp::Ordering {
            if err.is_some() {
                return std::cmp::Ordering::Equal;
            }
            let ord = a.0.cmp(&b.0);
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
            match order_values_for_sort(&a.1, &b.1, None, span, index, env_snapshot) {
                Ok(v) => v,
                Err(e) => {
                    err = Some(e);
                    std::cmp::Ordering::Equal
                }
            }
        };

    match plan.stability {
        SortStability::Stable => decorated.sort_by(&mut key_cmp),
        SortStability::Unstable => decorated.sort_unstable_by(&mut key_cmp),
    }
    if let Some(e) = err {
        return Err(e);
    }
    elems.extend(decorated.into_iter().map(|(_, value)| value));
    Ok(true)
}

fn sort_with_comparator_cache(
    elems: &mut Vec<Value>,
    compare_fn: &str,
    span: Span,
    index: &ProgramIndex,
    env_snapshot: &Env,
    stability: SortStability,
) -> Result<(), Diagnostic> {
    if elems.len() <= 1 {
        return Ok(());
    }
    let mut decorated: Vec<(u32, Value)> = std::mem::take(elems)
        .into_iter()
        .enumerate()
        .map(|(idx, value)| (idx as u32, value))
        .collect();
    let mut cache: HashMap<(u32, u32), std::cmp::Ordering> = HashMap::new();
    let mut err: Option<Diagnostic> = None;
    let mut cmp = |a: &(u32, Value), b: &(u32, Value)| -> std::cmp::Ordering {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        if a.0 == b.0 {
            return std::cmp::Ordering::Equal;
        }
        let (lo, hi, invert, left, right) = if a.0 < b.0 {
            (a.0, b.0, false, &a.1, &b.1)
        } else {
            (b.0, a.0, true, &b.1, &a.1)
        };
        let ord = if let Some(cached) = cache.get(&(lo, hi)).copied() {
            cached
        } else {
            match order_values_for_sort(left, right, Some(compare_fn), span, index, env_snapshot) {
                Ok(o) => {
                    cache.insert((lo, hi), o);
                    o
                }
                Err(e) => {
                    err = Some(e);
                    std::cmp::Ordering::Equal
                }
            }
        };
        if invert { ord.reverse() } else { ord }
    };

    match stability {
        SortStability::Stable => decorated.sort_by(&mut cmp),
        SortStability::Unstable => decorated.sort_unstable_by(&mut cmp),
    }
    if let Some(e) = err {
        return Err(e);
    }
    elems.extend(decorated.into_iter().map(|(_, value)| value));
    Ok(())
}

pub(super) fn sort_array_values(
    elems: &mut Vec<Value>,
    compare_fn: Option<&str>,
    span: Span,
    index: &ProgramIndex,
    env_snapshot: &Env,
    plan: SortPlan,
) -> Result<(), Diagnostic> {
    if elems.len() <= 1 {
        return Ok(());
    }
    if compare_fn.is_some() && plan.algorithm == SortAlgorithm::RadixOnly {
        return Err(type_error(
            "radix sorting does not accept custom comparators",
            span,
        ));
    }

    if compare_fn.is_none() {
        let radix_required = plan.algorithm == SortAlgorithm::RadixOnly;
        if try_radix_sort_values(elems, span, plan, radix_required)? {
            return Ok(());
        }
        if try_sort_with_cached_keys(elems, compare_fn, span, index, env_snapshot, plan)? {
            return Ok(());
        }
    } else if let Some(compare_name) = compare_fn {
        return sort_with_comparator_cache(
            elems,
            compare_name,
            span,
            index,
            env_snapshot,
            plan.stability,
        );
    }

    match plan.stability {
        SortStability::Stable => {
            let mut err: Option<Diagnostic> = None;
            elems.sort_by(|a, b| {
                if err.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                match order_values_for_sort(a, b, compare_fn, span, index, env_snapshot) {
                    Ok(ord) => ord,
                    Err(e) => {
                        err = Some(e);
                        std::cmp::Ordering::Equal
                    }
                }
            });
            if let Some(e) = err { Err(e) } else { Ok(()) }
        }
        SortStability::Unstable => {
            let mut cmp = |a: &Value, b: &Value| -> Result<std::cmp::Ordering, Diagnostic> {
                order_values_for_sort(a, b, compare_fn, span, index, env_snapshot)
            };
            block_quicksort_branch_buffer(elems.as_mut_slice(), &mut cmp)
        }
    }
}

fn block_quicksort_branch_buffer<F>(elems: &mut [Value], cmp: &mut F) -> Result<(), Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    const INSERTION_THRESHOLD: usize = 24;
    if elems.len() <= 1 {
        return Ok(());
    }
    let depth_limit = 2 * ((usize::BITS - elems.len().leading_zeros()) as usize);
    let mut stack = Vec::with_capacity(64);
    stack.push((0usize, elems.len(), depth_limit));
    while let Some((lo, hi, depth)) = stack.pop() {
        let len = hi.saturating_sub(lo);
        if len <= 1 {
            continue;
        }
        if len <= INSERTION_THRESHOLD {
            insertion_sort_range(elems, lo, hi, cmp)?;
            continue;
        }
        if depth == 0 {
            heap_sort_range(elems, lo, hi, cmp)?;
            continue;
        }
        let pivot_idx = choose_pivot_index(elems, lo, hi, cmp)?;
        let pivot_mid = block_partition_range(elems, lo, hi, pivot_idx, cmp)?;
        let left = (lo, pivot_mid);
        let right = (pivot_mid + 1, hi);
        let left_len = left.1.saturating_sub(left.0);
        let right_len = right.1.saturating_sub(right.0);
        if left_len > right_len {
            if left_len > 1 {
                stack.push((left.0, left.1, depth - 1));
            }
            if right_len > 1 {
                stack.push((right.0, right.1, depth - 1));
            }
        } else {
            if right_len > 1 {
                stack.push((right.0, right.1, depth - 1));
            }
            if left_len > 1 {
                stack.push((left.0, left.1, depth - 1));
            }
        }
    }
    Ok(())
}

fn insertion_sort_range<F>(
    elems: &mut [Value],
    lo: usize,
    hi: usize,
    cmp: &mut F,
) -> Result<(), Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    for i in (lo + 1)..hi {
        let mut j = i;
        while j > lo {
            if cmp(&elems[j], &elems[j - 1])? == std::cmp::Ordering::Less {
                elems.swap(j, j - 1);
                j -= 1;
            } else {
                break;
            }
        }
    }
    Ok(())
}

fn median_of_three_index<F>(
    elems: &[Value],
    lo: usize,
    hi: usize,
    cmp: &mut F,
) -> Result<usize, Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    let a = lo;
    let b = lo + (hi - lo) / 2;
    let c = hi - 1;
    median_of_three_indices(elems, a, b, c, cmp)
}

fn median_of_three_indices<F>(
    elems: &[Value],
    a: usize,
    b: usize,
    c: usize,
    cmp: &mut F,
) -> Result<usize, Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    let ab = cmp(&elems[a], &elems[b])?;
    if ab == std::cmp::Ordering::Less {
        let bc = cmp(&elems[b], &elems[c])?;
        if bc == std::cmp::Ordering::Less {
            return Ok(b);
        }
        let ac = cmp(&elems[a], &elems[c])?;
        if ac == std::cmp::Ordering::Less {
            Ok(c)
        } else {
            Ok(a)
        }
    } else {
        let bc = cmp(&elems[b], &elems[c])?;
        if bc == std::cmp::Ordering::Greater {
            return Ok(b);
        }
        let ac = cmp(&elems[a], &elems[c])?;
        if ac == std::cmp::Ordering::Greater {
            Ok(c)
        } else {
            Ok(a)
        }
    }
}

fn choose_pivot_index<F>(
    elems: &[Value],
    lo: usize,
    hi: usize,
    cmp: &mut F,
) -> Result<usize, Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    let len = hi - lo;
    if len < 128 {
        return median_of_three_index(elems, lo, hi, cmp);
    }

    let step = len / 8;
    let mid = lo + len / 2;
    let last = hi - 1;

    let a = median_of_three_indices(elems, lo, lo + step, lo + step * 2, cmp)?;
    let b = median_of_three_indices(elems, mid - step, mid, mid + step, cmp)?;
    let c = median_of_three_indices(elems, last - step * 2, last - step, last, cmp)?;
    median_of_three_indices(elems, a, b, c, cmp)
}

fn heap_sort_range<F>(
    elems: &mut [Value],
    lo: usize,
    hi: usize,
    cmp: &mut F,
) -> Result<(), Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    let len = hi.saturating_sub(lo);
    if len <= 1 {
        return Ok(());
    }
    for root in (0..(len / 2)).rev() {
        sift_down_range(elems, lo, root, len, cmp)?;
    }
    for end in (1..len).rev() {
        elems.swap(lo, lo + end);
        sift_down_range(elems, lo, 0, end, cmp)?;
    }
    Ok(())
}

fn sift_down_range<F>(
    elems: &mut [Value],
    lo: usize,
    mut root: usize,
    len: usize,
    cmp: &mut F,
) -> Result<(), Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    loop {
        let left = root * 2 + 1;
        if left >= len {
            return Ok(());
        }
        let right = left + 1;
        let mut child = left;
        if right < len && cmp(&elems[lo + left], &elems[lo + right])? == std::cmp::Ordering::Less {
            child = right;
        }
        if cmp(&elems[lo + root], &elems[lo + child])? == std::cmp::Ordering::Less {
            elems.swap(lo + root, lo + child);
            root = child;
        } else {
            return Ok(());
        }
    }
}

fn block_partition_range<F>(
    elems: &mut [Value],
    lo: usize,
    hi: usize,
    pivot_idx: usize,
    cmp: &mut F,
) -> Result<usize, Diagnostic>
where
    F: FnMut(&Value, &Value) -> Result<std::cmp::Ordering, Diagnostic>,
{
    const BLOCK: usize = 64;
    let pivot_slot = hi - 1;
    elems.swap(pivot_idx, pivot_slot);
    let mut store = lo;
    let mut i = lo;
    let mut less_offsets = [0usize; BLOCK];
    while i < pivot_slot {
        let end = (i + BLOCK).min(pivot_slot);
        let mut count = 0usize;
        for idx in i..end {
            if cmp(&elems[idx], &elems[pivot_slot])? == std::cmp::Ordering::Less {
                less_offsets[count] = idx;
                count += 1;
            }
        }
        for idx in less_offsets.iter().take(count).copied() {
            elems.swap(store, idx);
            store += 1;
        }
        i = end;
    }
    elems.swap(store, pivot_slot);
    Ok(store)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadixKeyKind {
    U32,
    U64,
    I32,
    I64,
}

fn try_radix_sort_values(
    elems: &mut Vec<Value>,
    span: Span,
    plan: SortPlan,
    required: bool,
) -> Result<bool, Diagnostic> {
    let Some(kind) = detect_radix_key_kind(elems) else {
        if required {
            return Err(type_error(
                "radix sort requires homogeneous array of i32/i64/u32/u64",
                span,
            ));
        }
        return Ok(false);
    };

    if plan.algorithm == SortAlgorithm::Auto && elems.len() < 32 {
        return Ok(false);
    }

    let bytes = match kind {
        RadixKeyKind::U32 | RadixKeyKind::I32 => 4usize,
        RadixKeyKind::U64 | RadixKeyKind::I64 => 8usize,
    };
    let mut pairs: Vec<(u64, Value)> = Vec::with_capacity(elems.len());
    for value in std::mem::take(elems) {
        let Some(key) = radix_key_for_value(kind, &value) else {
            return Err(type_error(
                "radix sort requires homogeneous array of i32/i64/u32/u64",
                span,
            ));
        };
        pairs.push((key, value));
    }

    match plan.stability {
        SortStability::Stable => radix_sort_pairs_lsd_stable(&mut pairs, bytes),
        SortStability::Unstable => {
            if bytes == 0 {
                pairs.sort_unstable_by_key(|(key, _)| *key);
            } else {
                radix_sort_pairs_msd_unstable(&mut pairs, (bytes as isize) - 1);
            }
        }
    }

    elems.extend(pairs.into_iter().map(|(_, value)| value));
    Ok(true)
}

fn detect_radix_key_kind(elems: &[Value]) -> Option<RadixKeyKind> {
    let first = elems.first()?;
    let kind = match first {
        Value::UInt { bits: 32, .. } => RadixKeyKind::U32,
        Value::UInt { bits: 64, .. } => RadixKeyKind::U64,
        Value::Int { bits: 32, .. } => RadixKeyKind::I32,
        Value::Int { bits: 64, .. } => RadixKeyKind::I64,
        _ => return None,
    };
    if elems
        .iter()
        .all(|value| radix_key_for_value(kind, value).is_some())
    {
        Some(kind)
    } else {
        None
    }
}

fn radix_key_for_value(kind: RadixKeyKind, value: &Value) -> Option<u64> {
    match (kind, value) {
        (RadixKeyKind::U32, Value::UInt { bits: 32, value }) => Some(*value as u32 as u64),
        (RadixKeyKind::U64, Value::UInt { bits: 64, value }) => Some(*value as u64),
        (RadixKeyKind::I32, Value::Int { bits: 32, value }) => {
            Some(((*value as i32 as u32) ^ 0x8000_0000u32) as u64)
        }
        (RadixKeyKind::I64, Value::Int { bits: 64, value }) => {
            Some(((*value as i64 as u64) ^ 0x8000_0000_0000_0000u64) as u64)
        }
        _ => None,
    }
}

fn radix_sort_pairs_lsd_stable(pairs: &mut Vec<(u64, Value)>, bytes: usize) {
    let n = pairs.len();
    if n <= 1 {
        return;
    }
    let mut current: Vec<Option<(u64, Value)>> = pairs.drain(..).map(Some).collect();
    let mut next: Vec<Option<(u64, Value)>> = (0..n).map(|_| None).collect();
    for byte in 0..bytes {
        let shift = (byte * 8) as u32;
        let mut counts = [0usize; 256];
        for item in &current {
            let key = item
                .as_ref()
                .expect("radix stable input slot should be populated")
                .0;
            counts[((key >> shift) & 0xFF) as usize] += 1;
        }
        let mut offsets = [0usize; 256];
        let mut running = 0usize;
        for (idx, count) in counts.iter().enumerate() {
            offsets[idx] = running;
            running += *count;
        }
        let mut positions = offsets;
        for item in &mut current {
            let pair = item
                .take()
                .expect("radix stable current slot should be populated");
            let bucket = ((pair.0 >> shift) & 0xFF) as usize;
            let target = positions[bucket];
            positions[bucket] += 1;
            next[target] = Some(pair);
        }
        std::mem::swap(&mut current, &mut next);
        for slot in &mut next {
            *slot = None;
        }
    }
    pairs.reserve(n);
    for item in current {
        pairs.push(item.expect("radix stable output slot should be populated"));
    }
}

fn radix_sort_pairs_msd_unstable(pairs: &mut [(u64, Value)], byte: isize) {
    const INSERTION_THRESHOLD: usize = 32;
    if pairs.len() <= INSERTION_THRESHOLD || byte < 0 {
        pairs.sort_unstable_by_key(|(key, _)| *key);
        return;
    }
    let shift = (byte as u32) * 8;
    let mut counts = [0usize; 256];
    for (key, _) in pairs.iter() {
        counts[((key >> shift) & 0xFF) as usize] += 1;
    }
    let mut offsets = [0usize; 256];
    let mut running = 0usize;
    for (bucket, count) in counts.iter().enumerate() {
        offsets[bucket] = running;
        running += *count;
    }
    let mut next = offsets;
    for bucket in 0..256usize {
        let end = offsets[bucket] + counts[bucket];
        let mut i = next[bucket];
        while i < end {
            let digit = ((pairs[i].0 >> shift) & 0xFF) as usize;
            if digit == bucket {
                i += 1;
                next[bucket] = i;
            } else {
                let target = next[digit];
                pairs.swap(i, target);
                next[digit] += 1;
            }
        }
    }
    if byte == 0 {
        return;
    }
    for bucket in 0..256usize {
        let start = offsets[bucket];
        let end = start + counts[bucket];
        if end.saturating_sub(start) > 1 {
            radix_sort_pairs_msd_unstable(&mut pairs[start..end], byte - 1);
        }
    }
}
