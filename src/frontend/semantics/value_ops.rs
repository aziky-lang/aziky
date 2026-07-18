fn apply_method_call(
    receiver: Value,
    name: &str,
    args: &[Value],
    span: Span,
    env: &Env,
) -> Result<Value, Diagnostic> {
    let receiver = resolve_receiver_value(receiver, env, span)?;

    if name == "len" {
        expect_method_arity(name, args, 0, span)?;
        return method_len(receiver, span);
    }
    if name == "char_count" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_count(receiver, span);
    }
    if name == "is_empty" {
        expect_method_arity(name, args, 0, span)?;
        return method_is_empty(receiver, span);
    }
    if name == "keys" {
        expect_method_arity(name, args, 0, span)?;
        return method_keys(receiver, span);
    }
    if name == "peek" {
        expect_method_arity(name, args, 0, span)?;
        return method_peek(receiver, span);
    }
    if name == "first" || name == "last" {
        expect_method_arity(name, args, 0, span)?;
        return method_list_boundary(receiver, name == "first", span);
    }
    if name == "trim" {
        expect_method_arity(name, args, 0, span)?;
        return method_trim(receiver, span);
    }
    if name == "to_upper" {
        expect_method_arity(name, args, 0, span)?;
        return method_to_upper(receiver, span);
    }
    if name == "to_lower" {
        expect_method_arity(name, args, 0, span)?;
        return method_to_lower(receiver, span);
    }
    if name == "not" {
        expect_method_arity(name, args, 0, span)?;
        return method_not(receiver, span);
    }
    if name == "abs" {
        expect_method_arity(name, args, 0, span)?;
        return method_abs(receiver, span);
    }
    if name == "is_positive" {
        expect_method_arity(name, args, 0, span)?;
        return method_is_positive(receiver, span);
    }
    if name == "is_negative" {
        expect_method_arity(name, args, 0, span)?;
        return method_is_negative(receiver, span);
    }
    if name == "is_alphabetic" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, char::is_alphabetic, name, span);
    }
    if name == "is_alphanumeric" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, char::is_alphanumeric, name, span);
    }
    if name == "is_numeric" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, char::is_numeric, name, span);
    }
    if name == "is_whitespace" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, char::is_whitespace, name, span);
    }
    if name == "is_uppercase" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, char::is_uppercase, name, span);
    }
    if name == "is_lowercase" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, char::is_lowercase, name, span);
    }
    if name == "is_ascii" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, |value| value.is_ascii(), name, span);
    }
    if name == "is_ascii_digit" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_predicate(receiver, |value| value.is_ascii_digit(), name, span);
    }
    if name == "to_ascii_upper" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_ascii_case(receiver, true, span);
    }
    if name == "to_ascii_lower" {
        expect_method_arity(name, args, 0, span)?;
        return method_char_ascii_case(receiver, false, span);
    }
    if name == "type_name" {
        expect_method_arity(name, args, 0, span)?;
        return Ok(Value::Str(value_type(&receiver).display()));
    }
    if name == "hash64" {
        expect_method_arity(name, args, 0, span)?;
        return method_hash64(receiver, span);
    }
    if name == "contains" {
        expect_method_arity(name, args, 1, span)?;
        return method_contains(receiver, args[0].clone(), span);
    }
    if name == "starts_with" {
        expect_method_arity(name, args, 1, span)?;
        return method_string_boundary(receiver, args[0].clone(), true, span);
    }
    if name == "ends_with" {
        expect_method_arity(name, args, 1, span)?;
        return method_string_boundary(receiver, args[0].clone(), false, span);
    }
    if name == "replace" {
        expect_method_arity(name, args, 2, span)?;
        return method_string_replace(receiver, args[0].clone(), args[1].clone(), span);
    }
    if name == "repeat" {
        expect_method_arity(name, args, 1, span)?;
        return method_string_repeat(receiver, args[0].clone(), span);
    }
    if name == "contains_key" {
        expect_method_arity(name, args, 1, span)?;
        return method_dict_contains_key(receiver, args[0].clone(), span);
    }
    if name == "get" {
        expect_method_arity(name, args, 1, span)?;
        return method_checked_get(receiver, args[0].clone(), span);
    }
    if name == "char_at" {
        expect_method_arity(name, args, 1, span)?;
        return method_char_at(receiver, args[0].clone(), span);
    }
    if name == "to_char_checked" {
        expect_method_arity(name, args, 0, span)?;
        return method_to_char_checked(receiver, span);
    }
    if let Some((signed, bits)) = parse_checked_integer_method(name) {
        expect_method_arity(name, args, 0, span)?;
        return method_parse_integer(receiver, signed, bits, name, span);
    }
    if let Some(bits) = parse_checked_float_method(name) {
        expect_method_arity(name, args, 0, span)?;
        return method_parse_float(receiver, bits, name, span);
    }
    if name == "parse_bool" {
        expect_method_arity(name, args, 0, span)?;
        return method_parse_bool(receiver, span);
    }
    if matches!(name, "is_some" | "is_none" | "is_ok" | "is_err") {
        expect_method_arity(name, args, 0, span)?;
        return method_enum_state_query(receiver, name, span);
    }
    if name == "unwrap_or" {
        expect_method_arity(name, args, 1, span)?;
        return method_unwrap_or(receiver, args[0].clone(), span);
    }
    if name == "min" {
        expect_method_arity(name, args, 1, span)?;
        return method_min_max(receiver, args[0].clone(), true, span);
    }
    if name == "max" {
        expect_method_arity(name, args, 1, span)?;
        return method_min_max(receiver, args[0].clone(), false, span);
    }
    if name == "clamp" {
        expect_method_arity(name, args, 2, span)?;
        return method_clamp(receiver, args[0].clone(), args[1].clone(), span);
    }

    if name == "to_str" || name == "to_string" {
        expect_method_arity(name, args, 0, span)?;
        return convert_to_string(receiver, span);
    }
    if name == "to_bool" {
        expect_method_arity(name, args, 0, span)?;
        return convert_to_bool(receiver, span);
    }
    if name == "to_byte" {
        expect_method_arity(name, args, 0, span)?;
        if matches!(&receiver, Value::Char(_)) {
            return Err(type_error(
                "char only converts explicitly with to_u32()",
                span,
            ));
        }
        return convert_to_int(receiver, false, 8, span);
    }

    if let Some(bits) = parse_int_method(name, "to_u") {
        expect_method_arity(name, args, 0, span)?;
        if matches!(&receiver, Value::Char(_)) && bits != 32 {
            return Err(type_error(
                "char only converts explicitly with to_u32()",
                span,
            ));
        }
        return convert_to_int(receiver, false, bits, span);
    }
    if let Some(bits) = parse_int_method(name, "to_i") {
        expect_method_arity(name, args, 0, span)?;
        if matches!(&receiver, Value::Char(_)) {
            return Err(type_error(
                "char only converts explicitly with to_u32()",
                span,
            ));
        }
        return convert_to_int(receiver, true, bits, span);
    }
    if let Some(bits) = parse_float_method(name) {
        expect_method_arity(name, args, 0, span)?;
        return convert_to_float(receiver, bits, span);
    }

    Err(type_error(
        format!("unknown method '{name}'").as_str(),
        span,
    ))
}

fn make_option(inner_type: TypeName, value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            type_args: vec![Some(inner_type)],
            payload: EnumPayloadValue::Tuple(vec![value]),
        },
        None => Value::Enum {
            name: "Option".to_string(),
            variant: "None".to_string(),
            type_args: vec![Some(inner_type)],
            payload: EnumPayloadValue::Unit,
        },
    }
}

fn make_parse_result(target_type: TypeName, parsed: Result<Value, Diagnostic>) -> Value {
    match parsed {
        Ok(value) => Value::Enum {
            name: "Result".to_string(),
            variant: "Ok".to_string(),
            type_args: vec![Some(target_type), Some(TypeName::String)],
            payload: EnumPayloadValue::Tuple(vec![value]),
        },
        Err(error) => Value::Enum {
            name: "Result".to_string(),
            variant: "Err".to_string(),
            type_args: vec![Some(target_type), Some(TypeName::String)],
            payload: EnumPayloadValue::Tuple(vec![Value::Str(error.message)]),
        },
    }
}

fn value_to_index(value: Value, span: Span) -> Result<Option<usize>, Diagnostic> {
    match value {
        Value::UInt { value, .. } => Ok(usize::try_from(value).ok()),
        Value::Int { value, .. } if value >= 0 => Ok(usize::try_from(value).ok()),
        Value::Int { .. } => Ok(None),
        _ => Err(type_error("index must be an integer", span)),
    }
}

fn method_checked_get(receiver: Value, key: Value, span: Span) -> Result<Value, Diagnostic> {
    match receiver {
        Value::Array { elem_type, elems } => {
            let value = value_to_index(key, span)?.and_then(|index| elems.get(index).cloned());
            Ok(make_option(elem_type, value))
        }
        Value::List { elem_type, elems } => {
            let value = value_to_index(key, span)?.and_then(|index| elems.get(index).cloned());
            Ok(make_option(elem_type, value))
        }
        Value::Dict {
            key_type,
            value_type,
            entries,
        } => {
            let key = dict_key_from_typed_value(key, &key_type, span)?;
            Ok(make_option(value_type, entries.get(&key).cloned()))
        }
        Value::Map {
            key_type,
            value_type,
            entries,
            ..
        } => {
            let key = dict_key_from_typed_value(key, &key_type, span)?;
            Ok(make_option(value_type, entries.get(&key).cloned()))
        }
        _ => Err(type_error(
            "get() is only available on arrays, lists, dictionaries, and maps",
            span,
        )),
    }
}

fn method_char_at(receiver: Value, index: Value, span: Span) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error("char_at() is only available on string", span));
    };
    let value = value_to_index(index, span)?.and_then(|index| text.chars().nth(index));
    Ok(make_option(TypeName::Char, value.map(Value::Char)))
}

fn method_to_char_checked(receiver: Value, span: Span) -> Result<Value, Diagnostic> {
    let scalar = match receiver {
        Value::UInt { value, .. } => u32::try_from(value).ok(),
        Value::Int { value, .. } if value >= 0 => u32::try_from(value).ok(),
        Value::Int { .. } => None,
        _ => {
            return Err(type_error(
                "to_char_checked() is only available on integer types",
                span,
            ));
        }
    };
    Ok(make_option(
        TypeName::Char,
        scalar.and_then(char::from_u32).map(Value::Char),
    ))
}

fn parse_checked_integer_method(name: &str) -> Option<(bool, u16)> {
    let suffix = name.strip_prefix("parse_")?;
    let (signed, digits) = match suffix.as_bytes().first().copied()? {
        b'i' => (true, &suffix[1..]),
        b'u' => (false, &suffix[1..]),
        _ => return None,
    };
    let bits = digits.parse().ok()?;
    matches!(bits, 8 | 16 | 32 | 64 | 128).then_some((signed, bits))
}

fn parse_checked_float_method(name: &str) -> Option<u16> {
    match name {
        "parse_f32" => Some(32),
        "parse_f64" => Some(64),
        _ => None,
    }
}

fn method_parse_integer(
    receiver: Value,
    signed: bool,
    bits: u16,
    method: &str,
    span: Span,
) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error(
            format!("{method}() is only available on string").as_str(),
            span,
        ));
    };
    let target = TypeName::Int { signed, bits };
    Ok(make_parse_result(
        target,
        parse_string_to_int(&text, signed, bits, span),
    ))
}

fn method_parse_float(
    receiver: Value,
    bits: u16,
    method: &str,
    span: Span,
) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error(
            format!("{method}() is only available on string").as_str(),
            span,
        ));
    };
    let target = TypeName::Float { bits };
    Ok(make_parse_result(
        target,
        parse_string_to_float(&text, bits, span),
    ))
}

fn method_parse_bool(receiver: Value, span: Span) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error("parse_bool() is only available on string", span));
    };
    Ok(make_parse_result(
        TypeName::Bool,
        parse_bool_string(&text, span),
    ))
}

fn method_enum_state_query(receiver: Value, method: &str, span: Span) -> Result<Value, Diagnostic> {
    let Value::Enum { name, variant, .. } = receiver else {
        return Err(type_error(
            format!("{method}() is only available on Option or Result").as_str(),
            span,
        ));
    };
    let answer = match (name.as_str(), method) {
        ("Option", "is_some") => variant == "Some",
        ("Option", "is_none") => variant == "None",
        ("Result", "is_ok") => variant == "Ok",
        ("Result", "is_err") => variant == "Err",
        _ => {
            return Err(type_error(
                format!("{method}() is not available on {name}").as_str(),
                span,
            ));
        }
    };
    Ok(Value::Bool(answer))
}

fn method_unwrap_or(receiver: Value, fallback: Value, span: Span) -> Result<Value, Diagnostic> {
    let Value::Enum {
        name,
        variant,
        type_args,
        payload,
    } = receiver
    else {
        return Err(type_error(
            "unwrap_or() is only available on Option or Result",
            span,
        ));
    };
    let target = type_args
        .first()
        .and_then(Clone::clone)
        .ok_or_else(|| type_error("unwrap_or() requires a concrete value type", span))?;
    match (name.as_str(), variant.as_str(), payload) {
        ("Option", "Some", EnumPayloadValue::Tuple(mut values))
        | ("Result", "Ok", EnumPayloadValue::Tuple(mut values)) => values
            .drain(..)
            .next()
            .ok_or_else(|| type_error("invalid value-carrying enum payload", span)),
        ("Option", "None", EnumPayloadValue::Unit)
        | ("Result", "Err", EnumPayloadValue::Tuple(_)) => coerce_value(fallback, &target, span),
        _ => Err(type_error(
            "unwrap_or() received an invalid Option or Result value",
            span,
        )),
    }
}

fn expect_method_arity(
    name: &str,
    args: &[Value],
    expected: usize,
    span: Span,
) -> Result<(), Diagnostic> {
    if args.len() == expected {
        return Ok(());
    }
    let noun = if expected == 1 {
        "argument"
    } else {
        "arguments"
    };
    Err(type_error(
        format!(
            "method '{name}' expects {expected} {noun}, got {}",
            args.len()
        )
        .as_str(),
        span,
    ))
}

fn resolve_receiver_value(value: Value, env: &Env, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Ref { target, .. } => {
            let binding = env.get(&target).ok_or_else(|| {
                Diagnostic::at_span(format!("unknown identifier '{target}'"), span)
            })?;
            Ok(binding.value.clone())
        }
        other => Ok(other),
    }
}

fn parse_int_method(name: &str, prefix: &str) -> Option<u16> {
    let bits = name.strip_prefix(prefix)?;
    match bits {
        "8" | "16" | "32" | "64" | "128" => bits.parse().ok(),
        _ => None,
    }
}

fn parse_float_method(name: &str) -> Option<u16> {
    let bits = name.strip_prefix("to_f")?;
    match bits {
        "32" | "64" => bits.parse().ok(),
        _ => None,
    }
}

fn fnv1a_update(state: &mut u64, bytes: &[u8]) {
    const FNV_PRIME: u64 = 0x100000001B3;
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(FNV_PRIME);
    }
}

fn hash_value64(value: &Value, span: Span) -> Result<u64, Diagnostic> {
    let mut state = 0xcbf29ce484222325u64;
    hash_value64_into(value, span, &mut state)?;
    Ok(state)
}

fn hash_value64_into(value: &Value, span: Span, state: &mut u64) -> Result<(), Diagnostic> {
    match value {
        Value::Bool(v) => {
            fnv1a_update(state, b"bool");
            fnv1a_update(state, &[*v as u8]);
        }
        Value::Str(text) => {
            fnv1a_update(state, b"str");
            fnv1a_update(state, text.as_bytes());
        }
        Value::Char(value) => {
            fnv1a_update(state, b"char");
            fnv1a_update(state, &u32::from(*value).to_le_bytes());
        }
        Value::Int { bits, value } => {
            fnv1a_update(state, b"int");
            fnv1a_update(state, &bits.to_le_bytes());
            fnv1a_update(state, &value.to_le_bytes());
        }
        Value::UInt { bits, value } => {
            fnv1a_update(state, b"uint");
            fnv1a_update(state, &bits.to_le_bytes());
            fnv1a_update(state, &value.to_le_bytes());
        }
        Value::Float { bits, value } => {
            if !value.is_finite() {
                return Err(type_error("non-finite float is not hashable", span));
            }
            fnv1a_update(state, b"float");
            fnv1a_update(state, &bits.to_le_bytes());
            fnv1a_update(state, &value.to_bits().to_le_bytes());
        }
        Value::Ref {
            target,
            mutable,
            inner,
        } => {
            fnv1a_update(state, b"ref");
            fnv1a_update(state, if *mutable { b"m" } else { b"s" });
            fnv1a_update(state, target.as_bytes());
            fnv1a_update(state, inner.display().as_bytes());
        }
        Value::Struct { name, fields } => {
            fnv1a_update(state, b"struct");
            fnv1a_update(state, name.as_bytes());
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            for key in keys {
                fnv1a_update(state, key.as_bytes());
                hash_value64_into(
                    fields.get(key).expect("field key should exist"),
                    span,
                    state,
                )?;
            }
        }
        Value::Enum {
            name,
            variant,
            type_args,
            payload,
        } => {
            fnv1a_update(state, b"enum");
            fnv1a_update(state, name.as_bytes());
            for arg in type_args {
                fnv1a_update(
                    state,
                    arg.as_ref()
                        .map(TypeName::display)
                        .unwrap_or_else(|| "_".to_string())
                        .as_bytes(),
                );
            }
            fnv1a_update(state, variant.as_bytes());
            hash_enum_payload(payload, span, state)?;
        }
        Value::Array { elems, .. } => {
            fnv1a_update(state, b"array");
            fnv1a_update(state, &(elems.len() as u64).to_le_bytes());
            for elem in elems {
                hash_value64_into(elem, span, state)?;
            }
        }
        Value::List { elems, .. } => {
            fnv1a_update(state, b"list");
            fnv1a_update(state, &(elems.len() as u64).to_le_bytes());
            for elem in elems {
                hash_value64_into(elem, span, state)?;
            }
        }
        Value::Dict { entries, .. } => {
            fnv1a_update(state, b"dict");
            fnv1a_update(state, &(entries.len() as u64).to_le_bytes());
            for (key, value) in entries {
                fnv1a_update(state, key.as_bytes());
                hash_value64_into(value, span, state)?;
            }
        }
        Value::Map { entries, .. } => {
            fnv1a_update(state, b"map");
            fnv1a_update(state, &(entries.len() as u64).to_le_bytes());
            for (key, value) in entries {
                fnv1a_update(state, key.as_bytes());
                hash_value64_into(value, span, state)?;
            }
        }
    }
    Ok(())
}

fn hash_enum_payload(
    payload: &EnumPayloadValue,
    span: Span,
    state: &mut u64,
) -> Result<(), Diagnostic> {
    match payload {
        EnumPayloadValue::Unit => fnv1a_update(state, b"unit"),
        EnumPayloadValue::Tuple(values) => {
            fnv1a_update(state, b"tuple");
            for value in values {
                hash_value64_into(value, span, state)?;
            }
        }
        EnumPayloadValue::Named(fields) => {
            fnv1a_update(state, b"named");
            let mut names: Vec<&String> = fields.keys().collect();
            names.sort_unstable();
            for name in names {
                fnv1a_update(state, name.as_bytes());
                hash_value64_into(
                    fields.get(name).expect("enum payload field should exist"),
                    span,
                    state,
                )?;
            }
        }
    }
    Ok(())
}

fn dict_key_repr(value: &Value, span: Span) -> Result<String, Diagnostic> {
    match value {
        Value::Bool(v) => Ok(format!("b:{}", u8::from(*v))),
        Value::Str(v) => Ok(v.clone()),
        Value::Char(v) => Ok(format!("c:{:x}", u32::from(*v))),
        Value::Int { bits, value } => Ok(format!("i{}:{value}", bits)),
        Value::UInt { bits, value } => Ok(format!("u{}:{value}", bits)),
        Value::Float { bits, value } => {
            if !value.is_finite() {
                return Err(type_error(
                    "non-finite float is not valid as dictionary key",
                    span,
                ));
            }
            Ok(format!("f{}:{:016x}", bits, value.to_bits()))
        }
        Value::Enum {
            name,
            variant,
            type_args,
            payload,
        } => Ok(format!(
            "e:{name}<{}>:{variant}:{}",
            type_args
                .iter()
                .map(|arg| arg
                    .as_ref()
                    .map(TypeName::display)
                    .unwrap_or_else(|| "_".to_string()))
                .collect::<Vec<_>>()
                .join(","),
            enum_payload_key_repr(payload, span)?
        )),
        Value::Struct { name, fields } => {
            let mut out = String::new();
            out.push_str("s:");
            out.push_str(name);
            out.push('{');
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_unstable();
            for (idx, key) in keys.iter().enumerate() {
                if idx != 0 {
                    out.push(';');
                }
                out.push_str(key.as_str());
                out.push('=');
                out.push_str(&dict_key_repr(
                    fields.get(*key).expect("field key should exist"),
                    span,
                )?);
            }
            out.push('}');
            Ok(out)
        }
        Value::Array { elems, .. } => {
            let mut out = String::new();
            out.push_str("a:[");
            for (idx, value) in elems.iter().enumerate() {
                if idx != 0 {
                    out.push(',');
                }
                out.push_str(&dict_key_repr(value, span)?);
            }
            out.push(']');
            Ok(out)
        }
        Value::List { elems, .. } => {
            let mut out = String::from("l:[");
            for (idx, value) in elems.iter().enumerate() {
                if idx != 0 {
                    out.push(',');
                }
                out.push_str(&dict_key_repr(value, span)?);
            }
            out.push(']');
            Ok(out)
        }
        Value::Dict { entries, .. } => {
            let mut out = String::new();
            out.push_str("d:{");
            for (idx, (key, value)) in entries.iter().enumerate() {
                if idx != 0 {
                    out.push(';');
                }
                out.push_str(key);
                out.push('=');
                out.push_str(&dict_key_repr(value, span)?);
            }
            out.push('}');
            Ok(out)
        }
        Value::Map { entries, .. } => {
            let mut out = String::from("m:{");
            for (idx, (key, value)) in entries.iter().enumerate() {
                if idx != 0 {
                    out.push(';');
                }
                out.push_str(key);
                out.push('=');
                out.push_str(&dict_key_repr(value, span)?);
            }
            out.push('}');
            Ok(out)
        }
        Value::Ref { .. } => Err(type_error(
            "reference values cannot be used as dictionary keys",
            span,
        )),
    }
}

fn enum_payload_key_repr(payload: &EnumPayloadValue, span: Span) -> Result<String, Diagnostic> {
    match payload {
        EnumPayloadValue::Unit => Ok("unit".to_string()),
        EnumPayloadValue::Tuple(values) => {
            let mut out = String::from("tuple(");
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                out.push_str(&dict_key_repr(value, span)?);
            }
            out.push(')');
            Ok(out)
        }
        EnumPayloadValue::Named(fields) => {
            let mut names: Vec<&String> = fields.keys().collect();
            names.sort_unstable();
            let mut out = String::from("named{");
            for (index, name) in names.iter().enumerate() {
                if index != 0 {
                    out.push(';');
                }
                out.push_str(name);
                out.push('=');
                out.push_str(&dict_key_repr(
                    fields.get(*name).expect("enum payload field should exist"),
                    span,
                )?);
            }
            out.push('}');
            Ok(out)
        }
    }
}

fn dict_key_from_typed_value(
    value: Value,
    key_ty: &TypeName,
    span: Span,
) -> Result<String, Diagnostic> {
    let coerced = coerce_value(value, key_ty, span)?;
    dict_key_repr(&coerced, span)
}

fn convert_to_string(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::Str(text)),
        Value::Int { value, .. } => Ok(Value::Str(value.to_string())),
        Value::UInt { value, .. } => Ok(Value::Str(value.to_string())),
        Value::Float { value, .. } => Ok(Value::Str(value.to_string())),
        Value::Bool(value) => Ok(Value::Str(value.to_string())),
        Value::Char(value) => Ok(Value::Str(value.to_string())),
        Value::Enum {
            name,
            variant,
            payload,
            ..
        } => Ok(Value::Str(format_enum_value(
            &name, &variant, &payload, span,
        )?)),
        Value::Ref { .. } => Err(type_error("cannot convert reference to string", span)),
        Value::Struct { .. } => Err(type_error("cannot convert struct to string", span)),
        Value::Array { .. } => Err(type_error("cannot convert array to string", span)),
        Value::List { .. } => Err(type_error("cannot convert list to string", span)),
        Value::Dict { .. } => Err(type_error("cannot convert dictionary to string", span)),
        Value::Map { .. } => Err(type_error("cannot convert map to string", span)),
    }
}

fn convert_to_bool(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Bool(value) => Ok(Value::Bool(value)),
        Value::Str(text) => parse_bool_string(&text, span),
        Value::Int { value, .. } => match value {
            0 => Ok(Value::Bool(false)),
            1 => Ok(Value::Bool(true)),
            _ => Err(type_error("integer must be 0 or 1 for to_bool()", span)),
        },
        Value::UInt { value, .. } => match value {
            0 => Ok(Value::Bool(false)),
            1 => Ok(Value::Bool(true)),
            _ => Err(type_error("integer must be 0 or 1 for to_bool()", span)),
        },
        Value::Float { value, .. } => {
            if !value.is_finite() {
                return Err(type_error("float out of range", span));
            }
            if value == 0.0 {
                Ok(Value::Bool(false))
            } else if value == 1.0 {
                Ok(Value::Bool(true))
            } else {
                Err(type_error("float must be 0.0 or 1.0 for to_bool()", span))
            }
        }
        Value::Char(_) => Err(type_error("cannot convert char to bool", span)),
        Value::Ref { .. } => Err(type_error("cannot convert reference to bool", span)),
        Value::Struct { .. } => Err(type_error("cannot convert struct to bool", span)),
        Value::Enum { .. } => Err(type_error("cannot convert enum to bool", span)),
        Value::Array { .. } => Err(type_error("cannot convert array to bool", span)),
        Value::List { .. } => Err(type_error("cannot convert list to bool", span)),
        Value::Dict { .. } => Err(type_error("cannot convert dictionary to bool", span)),
        Value::Map { .. } => Err(type_error("cannot convert map to bool", span)),
    }
}

fn parse_bool_string(text: &str, span: Span) -> Result<Value, Diagnostic> {
    let normalized = text.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "true" | "1" => Ok(Value::Bool(true)),
        "false" | "0" => Ok(Value::Bool(false)),
        _ => Err(Diagnostic::at_span("invalid bool literal", span)),
    }
}

fn stringify_value_for_error(value: Value, span: Span) -> Result<String, Diagnostic> {
    match value {
        Value::Str(text) => Ok(text),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Int { value, .. } => Ok(value.to_string()),
        Value::UInt { value, .. } => Ok(value.to_string()),
        Value::Float { value, .. } => Ok(value.to_string()),
        Value::Char(value) => Ok(value.to_string()),
        Value::Enum {
            name,
            variant,
            payload,
            ..
        } => format_enum_value(&name, &variant, &payload, span),
        Value::Ref { .. } => Err(type_error("error message cannot be a reference", span)),
        Value::Struct { .. } => Err(type_error("error message cannot be a struct", span)),
        Value::Array { .. } => Err(type_error("error message cannot be an array", span)),
        Value::List { .. } => Err(type_error("error message cannot be a list", span)),
        Value::Dict { .. } => Err(type_error("error message cannot be a dictionary", span)),
        Value::Map { .. } => Err(type_error("error message cannot be a map", span)),
    }
}

fn format_enum_value(
    name: &str,
    variant: &str,
    payload: &EnumPayloadValue,
    span: Span,
) -> Result<String, Diagnostic> {
    let mut out = format!("{name}::{variant}");
    match payload {
        EnumPayloadValue::Unit => {}
        EnumPayloadValue::Tuple(values) => {
            out.push('(');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_value_for_debug(value, span)?);
            }
            out.push(')');
        }
        EnumPayloadValue::Named(fields) => {
            out.push_str(" { ");
            let mut names: Vec<&String> = fields.keys().collect();
            names.sort_unstable();
            for (index, field_name) in names.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(field_name);
                out.push_str(": ");
                out.push_str(&format_value_for_debug(
                    fields
                        .get(*field_name)
                        .expect("enum payload field should exist"),
                    span,
                )?);
            }
            out.push_str(" }");
        }
    }
    Ok(out)
}

fn format_value_for_debug(value: &Value, span: Span) -> Result<String, Diagnostic> {
    match value {
        Value::Bool(value) => Ok(value.to_string()),
        Value::Str(value) => Ok(format!("\"{value}\"")),
        Value::Char(value) => Ok(format!("'{value}'")),
        Value::Int { value, .. } => Ok(value.to_string()),
        Value::UInt { value, .. } => Ok(value.to_string()),
        Value::Float { value, .. } => Ok(value.to_string()),
        Value::Enum {
            name,
            variant,
            payload,
            ..
        } => format_enum_value(name, variant, payload, span),
        Value::Struct { name, fields } => {
            let mut names: Vec<&String> = fields.keys().collect();
            names.sort_unstable();
            let mut out = format!("{name} {{ ");
            for (index, field_name) in names.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(field_name);
                out.push_str(": ");
                out.push_str(&format_value_for_debug(
                    fields.get(*field_name).expect("struct field should exist"),
                    span,
                )?);
            }
            out.push_str(" }");
            Ok(out)
        }
        Value::Array { elems, .. } => {
            let mut out = String::from("[");
            for (index, elem) in elems.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_value_for_debug(elem, span)?);
            }
            out.push(']');
            Ok(out)
        }
        Value::List { elems, .. } => {
            let mut out = String::from("list[");
            for (index, elem) in elems.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(&format_value_for_debug(elem, span)?);
            }
            out.push(']');
            Ok(out)
        }
        Value::Dict { entries, .. } => {
            let mut out = String::from("{");
            for (index, (key, value)) in entries.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&format_value_for_debug(value, span)?);
            }
            out.push('}');
            Ok(out)
        }
        Value::Map { entries, .. } => {
            let mut out = String::from("map{");
            for (index, (key, value)) in entries.iter().enumerate() {
                if index != 0 {
                    out.push_str(", ");
                }
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&format_value_for_debug(value, span)?);
            }
            out.push('}');
            Ok(out)
        }
        Value::Ref { .. } => Err(type_error("cannot format reference value", span)),
    }
}

fn method_len(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::UInt {
            bits: 64,
            value: text.len() as u128,
        }),
        Value::Array { elems, .. } => Ok(Value::UInt {
            bits: 64,
            value: elems.len() as u128,
        }),
        Value::List { elems, .. } => Ok(Value::UInt {
            bits: 64,
            value: elems.len() as u128,
        }),
        Value::Dict { entries, .. } => Ok(Value::UInt {
            bits: 64,
            value: entries.len() as u128,
        }),
        Value::Map { entries, .. } => Ok(Value::UInt {
            bits: 64,
            value: entries.len() as u128,
        }),
        _ => Err(type_error(
            "len() is only available on string, array, and dictionary",
            span,
        )),
    }
}

fn method_char_count(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::UInt {
            bits: 64,
            value: text.chars().count() as u128,
        }),
        _ => Err(type_error("char_count() is only available on string", span)),
    }
}

fn string_pattern(value: Value, method: &str, span: Span) -> Result<String, Diagnostic> {
    match value {
        Value::Str(value) => Ok(value),
        Value::Char(value) => Ok(value.to_string()),
        other => Err(type_error(
            format!(
                "{method}() expects a string or char pattern, got {}",
                value_type(&other).display()
            )
            .as_str(),
            span,
        )),
    }
}

fn method_contains(receiver: Value, needle: Value, span: Span) -> Result<Value, Diagnostic> {
    match receiver {
        Value::Str(text) => {
            let needle = string_pattern(needle, "contains", span)?;
            Ok(Value::Bool(text.contains(&needle)))
        }
        Value::Array { elem_type, elems } => {
            let needle = coerce_value(needle, &elem_type, span)?;
            for elem in elems {
                if value_partial_cmp(&elem, &needle, span)? == std::cmp::Ordering::Equal {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        Value::List { elem_type, elems } => {
            let needle = coerce_value(needle, &elem_type, span)?;
            for elem in elems {
                if value_partial_cmp(&elem, &needle, span)? == std::cmp::Ordering::Equal {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        other => Err(type_error(
            format!(
                "contains() is only available on string and array, got {}",
                value_type(&other).display()
            )
            .as_str(),
            span,
        )),
    }
}

fn method_string_boundary(
    receiver: Value,
    pattern: Value,
    starts: bool,
    span: Span,
) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error(
            "starts_with() and ends_with() are only available on string",
            span,
        ));
    };
    let method = if starts { "starts_with" } else { "ends_with" };
    let pattern = string_pattern(pattern, method, span)?;
    Ok(Value::Bool(if starts {
        text.starts_with(&pattern)
    } else {
        text.ends_with(&pattern)
    }))
}

fn method_string_replace(
    receiver: Value,
    from: Value,
    to: Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error("replace() is only available on string", span));
    };
    let from = string_pattern(from, "replace", span)?;
    let to = string_pattern(to, "replace", span)?;
    Ok(Value::Str(text.replace(&from, &to)))
}

fn method_string_repeat(receiver: Value, count: Value, span: Span) -> Result<Value, Diagnostic> {
    let Value::Str(text) = receiver else {
        return Err(type_error("repeat() is only available on string", span));
    };
    let count =
        match count {
            Value::UInt { value, .. } => usize::try_from(value)
                .map_err(|_| type_error("repeat() count is too large", span))?,
            Value::Int { value, .. } if value >= 0 => usize::try_from(value)
                .map_err(|_| type_error("repeat() count is too large", span))?,
            Value::Int { .. } => {
                return Err(type_error("repeat() count cannot be negative", span));
            }
            other => {
                return Err(type_error(
                    format!(
                        "repeat() expects an integer count, got {}",
                        value_type(&other).display()
                    )
                    .as_str(),
                    span,
                ));
            }
        };
    let capacity = text
        .len()
        .checked_mul(count)
        .ok_or_else(|| type_error("repeat() result is too large", span))?;
    let mut result = String::new();
    result
        .try_reserve(capacity)
        .map_err(|_| type_error("repeat() could not allocate its result", span))?;
    for _ in 0..count {
        result.push_str(&text);
    }
    Ok(Value::Str(result))
}

fn method_dict_contains_key(receiver: Value, key: Value, span: Span) -> Result<Value, Diagnostic> {
    let (key_type, entries) = match receiver {
        Value::Dict {
            key_type, entries, ..
        }
        | Value::Map {
            key_type, entries, ..
        } => (key_type, entries),
        _ => {
            return Err(type_error(
                "contains_key() is only available on dictionary and map",
                span,
            ));
        }
    };
    let key = dict_key_from_typed_value(key, &key_type, span)?;
    Ok(Value::Bool(entries.contains_key(&key)))
}

fn method_min_max(
    receiver: Value,
    other: Value,
    minimum: bool,
    span: Span,
) -> Result<Value, Diagnostic> {
    let ty = value_type(&receiver);
    let other = coerce_value(other, &ty, span)?;
    let ordering = value_partial_cmp(&receiver, &other, span)?;
    let take_receiver = if minimum {
        ordering != std::cmp::Ordering::Greater
    } else {
        ordering != std::cmp::Ordering::Less
    };
    Ok(if take_receiver { receiver } else { other })
}

fn method_clamp(
    receiver: Value,
    minimum: Value,
    maximum: Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let ty = value_type(&receiver);
    let minimum = coerce_value(minimum, &ty, span)?;
    let maximum = coerce_value(maximum, &ty, span)?;
    if value_partial_cmp(&minimum, &maximum, span)? == std::cmp::Ordering::Greater {
        return Err(type_error(
            "clamp() minimum must not be greater than maximum",
            span,
        ));
    }
    if value_partial_cmp(&receiver, &minimum, span)? == std::cmp::Ordering::Less {
        return Ok(minimum);
    }
    if value_partial_cmp(&receiver, &maximum, span)? == std::cmp::Ordering::Greater {
        return Ok(maximum);
    }
    Ok(receiver)
}

fn method_is_empty(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::Bool(text.is_empty())),
        Value::Array { elems, .. } => Ok(Value::Bool(elems.is_empty())),
        Value::List { elems, .. } => Ok(Value::Bool(elems.is_empty())),
        Value::Dict { entries, .. } => Ok(Value::Bool(entries.is_empty())),
        Value::Map { entries, .. } => Ok(Value::Bool(entries.is_empty())),
        _ => Err(type_error(
            "is_empty() is only available on string, array, and dictionary",
            span,
        )),
    }
}

fn method_peek(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Array { elems, .. } => elems
            .last()
            .cloned()
            .ok_or_else(|| type_error("peek() on empty array", span)),
        Value::List { elem_type, elems } => Ok(make_option(elem_type, elems.last().cloned())),
        _ => Err(type_error("peek() is only available on array", span)),
    }
}

fn method_list_boundary(value: Value, first: bool, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::List { elem_type, elems } => {
            let value = if first { elems.first() } else { elems.last() }.cloned();
            Ok(make_option(elem_type, value))
        }
        _ => Err(type_error(
            "first() and last() are only available on lists",
            span,
        )),
    }
}

fn method_keys(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Dict { entries, .. } => {
            let mut elems = Vec::with_capacity(entries.len());
            for key in entries.keys() {
                elems.push(Value::Str(key.clone()));
            }
            Ok(Value::Array {
                elem_type: TypeName::String,
                elems,
            })
        }
        Value::Map { key_type, keys, .. } => Ok(Value::Array {
            elem_type: key_type,
            elems: keys.into_values().collect(),
        }),
        _ => Err(type_error(
            "keys() is only available on dictionary and map",
            span,
        )),
    }
}

fn method_hash64(value: Value, span: Span) -> Result<Value, Diagnostic> {
    let hashed = hash_value64(&value, span)?;
    Ok(Value::UInt {
        bits: 64,
        value: hashed as u128,
    })
}

fn method_trim(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::Str(text.trim().to_string())),
        _ => Err(type_error("trim() is only available on string", span)),
    }
}

fn method_to_upper(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::Str(text.to_ascii_uppercase())),
        Value::Char(value) => Ok(Value::Str(value.to_uppercase().collect())),
        _ => Err(type_error(
            "to_upper() is only available on string and char",
            span,
        )),
    }
}

fn method_to_lower(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Str(text) => Ok(Value::Str(text.to_ascii_lowercase())),
        Value::Char(value) => Ok(Value::Str(value.to_lowercase().collect())),
        _ => Err(type_error(
            "to_lower() is only available on string and char",
            span,
        )),
    }
}

fn method_not(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Bool(value) => Ok(Value::Bool(!value)),
        _ => Err(type_error("not() is only available on bool", span)),
    }
}

fn method_abs(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Int { bits, value } => {
            let value = value
                .checked_abs()
                .ok_or_else(|| type_error("integer overflow in abs()", span))?;
            Ok(Value::Int { bits, value })
        }
        Value::Float { bits, value } => Ok(Value::Float {
            bits,
            value: value.abs(),
        }),
        _ => Err(type_error(
            "abs() is only available on signed numbers",
            span,
        )),
    }
}

fn method_is_positive(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Int { value, .. } => Ok(Value::Bool(value > 0)),
        Value::UInt { value, .. } => Ok(Value::Bool(value > 0)),
        Value::Float { value, .. } => Ok(Value::Bool(value > 0.0)),
        _ => Err(type_error(
            "is_positive() is only available on numeric types",
            span,
        )),
    }
}

fn method_is_negative(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Int { value, .. } => Ok(Value::Bool(value < 0)),
        Value::Float { value, .. } => Ok(Value::Bool(value < 0.0)),
        Value::UInt { .. } => Ok(Value::Bool(false)),
        _ => Err(type_error(
            "is_negative() is only available on numeric types",
            span,
        )),
    }
}

fn method_char_predicate(
    value: Value,
    predicate: impl FnOnce(char) -> bool,
    method: &str,
    span: Span,
) -> Result<Value, Diagnostic> {
    match value {
        Value::Char(value) => Ok(Value::Bool(predicate(value))),
        _ => Err(type_error(
            format!("{method}() is only available on char").as_str(),
            span,
        )),
    }
}

fn method_char_ascii_case(value: Value, uppercase: bool, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Char(value) => Ok(Value::Char(if uppercase {
            value.to_ascii_uppercase()
        } else {
            value.to_ascii_lowercase()
        })),
        _ => Err(type_error(
            "ASCII case conversion is only available on char",
            span,
        )),
    }
}

fn convert_to_int(value: Value, signed: bool, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Int { value, .. } => convert_int_value(value, signed, bits, span),
        Value::UInt { value, .. } => convert_uint_value(value, signed, bits, span),
        Value::Float { value, .. } => convert_float_to_int(value, signed, bits, span),
        Value::Bool(value) => {
            let numeric = if value { 1 } else { 0 };
            convert_int_value(numeric, signed, bits, span)
        }
        Value::Char(value) => convert_uint_value(u128::from(u32::from(value)), signed, bits, span),
        Value::Str(text) => parse_string_to_int(&text, signed, bits, span),
        Value::Ref { .. } => Err(type_error("cannot convert reference to integer", span)),
        Value::Struct { .. } => Err(type_error("cannot convert struct to integer", span)),
        Value::Enum { .. } => Err(type_error("cannot convert enum to integer", span)),
        Value::Array { .. } => Err(type_error("cannot convert array to integer", span)),
        Value::List { .. } => Err(type_error("cannot convert list to integer", span)),
        Value::Dict { .. } => Err(type_error("cannot convert dictionary to integer", span)),
        Value::Map { .. } => Err(type_error("cannot convert map to integer", span)),
    }
}

fn convert_to_float(value: Value, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Float { value, .. } => coerce_float_bits(value, bits, span),
        Value::Int { value, .. } => {
            let value = value as f64;
            coerce_float_bits(value, bits, span)
        }
        Value::UInt { value, .. } => {
            let value = value as f64;
            coerce_float_bits(value, bits, span)
        }
        Value::Bool(value) => {
            let value = if value { 1.0 } else { 0.0 };
            coerce_float_bits(value, bits, span)
        }
        Value::Char(_) => Err(type_error("cannot convert char to float", span)),
        Value::Str(text) => parse_string_to_float(&text, bits, span),
        Value::Ref { .. } => Err(type_error("cannot convert reference to float", span)),
        Value::Struct { .. } => Err(type_error("cannot convert struct to float", span)),
        Value::Enum { .. } => Err(type_error("cannot convert enum to float", span)),
        Value::Array { .. } => Err(type_error("cannot convert array to float", span)),
        Value::List { .. } => Err(type_error("cannot convert list to float", span)),
        Value::Dict { .. } => Err(type_error("cannot convert dictionary to float", span)),
        Value::Map { .. } => Err(type_error("cannot convert map to float", span)),
    }
}

fn convert_int_value(
    value: i128,
    signed: bool,
    bits: u16,
    span: Span,
) -> Result<Value, Diagnostic> {
    if signed {
        check_int_range(value, bits, span)?;
        Ok(Value::Int { bits, value })
    } else {
        if value < 0 {
            return Err(type_error("cannot convert negative to unsigned", span));
        }
        let value = u128::try_from(value).map_err(|_| type_error("integer out of range", span))?;
        check_uint_range(value, bits, span)?;
        Ok(Value::UInt { bits, value })
    }
}

fn convert_uint_value(
    value: u128,
    signed: bool,
    bits: u16,
    span: Span,
) -> Result<Value, Diagnostic> {
    if signed {
        let value = i128::try_from(value).map_err(|_| type_error("integer out of range", span))?;
        check_int_range(value, bits, span)?;
        Ok(Value::Int { bits, value })
    } else {
        check_uint_range(value, bits, span)?;
        Ok(Value::UInt { bits, value })
    }
}

fn convert_float_to_int(
    value: f64,
    signed: bool,
    bits: u16,
    span: Span,
) -> Result<Value, Diagnostic> {
    if !value.is_finite() {
        return Err(type_error("float out of range", span));
    }
    if value.fract() != 0.0 {
        return Err(type_error("cannot convert non-integer float to int", span));
    }

    if signed {
        let (min, max) = int_range_limits(bits);
        if value < min || value > max {
            return Err(type_error("integer out of range", span));
        }
        Ok(Value::Int {
            bits,
            value: value as i128,
        })
    } else {
        let max = uint_range_limit(bits);
        if value < 0.0 || value > max {
            return Err(type_error("integer out of range", span));
        }
        Ok(Value::UInt {
            bits,
            value: value as u128,
        })
    }
}

fn parse_string_to_int(
    text: &str,
    signed: bool,
    bits: u16,
    span: Span,
) -> Result<Value, Diagnostic> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Diagnostic::at_span(
            "cannot parse empty string as integer",
            span,
        ));
    }
    if signed {
        let value: i128 = trimmed
            .parse()
            .map_err(|_| Diagnostic::at_span(format!("invalid i{bits} literal"), span))?;
        check_int_range(value, bits, span)?;
        Ok(Value::Int { bits, value })
    } else {
        if trimmed.starts_with('-') {
            return Err(type_error(
                "cannot parse negative as unsigned integer",
                span,
            ));
        }
        let value: u128 = trimmed
            .parse()
            .map_err(|_| Diagnostic::at_span(format!("invalid u{bits} literal"), span))?;
        check_uint_range(value, bits, span)?;
        Ok(Value::UInt { bits, value })
    }
}

fn parse_string_to_float(text: &str, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Diagnostic::at_span(
            "cannot parse empty string as float",
            span,
        ));
    }
    let value: f64 = trimmed
        .parse()
        .map_err(|_| Diagnostic::at_span(format!("invalid f{bits} literal"), span))?;
    if !value.is_finite() {
        return Err(type_error("float out of range", span));
    }
    coerce_float_bits(value, bits, span)
}

fn coerce_float_bits(value: f64, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    match bits {
        32 => {
            let v32 = value as f32;
            if !v32.is_finite() {
                return Err(type_error("float out of range for f32", span));
            }
            Ok(Value::Float {
                bits: 32,
                value: v32 as f64,
            })
        }
        64 => Ok(Value::Float { bits: 64, value }),
        _ => Err(type_error("unsupported float width", span)),
    }
}

fn int_range_limits(bits: u16) -> (f64, f64) {
    if bits == 128 {
        (i128::MIN as f64, i128::MAX as f64)
    } else {
        let max = (1_i128 << (bits - 1)) - 1;
        let min = -(1_i128 << (bits - 1));
        (min as f64, max as f64)
    }
}

fn uint_range_limit(bits: u16) -> f64 {
    if bits == 128 {
        u128::MAX as f64
    } else {
        let max = (1_u128 << bits) - 1;
        max as f64
    }
}

fn borrow_value(
    expr: &Expr,
    env: &mut Env,
    mutable: bool,
    span: Span,
) -> Result<Value, Diagnostic> {
    match expr {
        Expr::Ident { name, .. } => {
            let binding = env
                .get_mut(name)
                .ok_or_else(|| Diagnostic::at_span(format!("unknown identifier '{name}'"), span))?;

            if mutable {
                if !binding.mutable {
                    return Err(type_error(
                        format!("cannot take &mut of immutable '{name}'").as_str(),
                        span,
                    ));
                }
                if binding.mut_borrowed || binding.shared_borrows > 0 {
                    return Err(type_error(
                        format!("'{name}' already borrowed").as_str(),
                        span,
                    ));
                }
                binding.mut_borrowed = true;
            } else {
                if binding.mut_borrowed {
                    return Err(type_error(
                        format!("'{name}' already mutably borrowed").as_str(),
                        span,
                    ));
                }
                binding.shared_borrows += 1;
            }

            Ok(Value::Ref {
                target: name.clone(),
                mutable,
                inner: binding.ty.clone(),
            })
        }
        _ => Err(type_error("borrows must target identifiers", span)),
    }
}

fn release_borrows(bindings: &[(String, Binding)], env: &mut Env) -> Result<(), Diagnostic> {
    let mut dropped: HashMap<String, ()> = HashMap::new();
    for (name, _) in bindings {
        dropped.insert(name.clone(), ());
    }

    for (_, binding) in bindings {
        if let Value::Ref {
            target, mutable, ..
        } = &binding.value
        {
            if dropped.contains_key(target) {
                continue;
            }
            let target_binding = env.get_mut(target).ok_or_else(|| {
                Diagnostic::new(
                    format!("internal error: missing borrow target '{target}'"),
                    0,
                    0,
                )
            })?;
            if *mutable {
                target_binding.mut_borrowed = false;
            } else if target_binding.shared_borrows > 0 {
                target_binding.shared_borrows -= 1;
            }
        }
    }
    Ok(())
}

fn parse_number_literal(literal: &str, span: Span) -> Result<Value, Diagnostic> {
    let (num_part, suffix) = split_literal(literal);
    let is_float = num_part.contains('.');

    match suffix.as_deref() {
        Some("byte") => parse_uint_with_bits(num_part, 8, span),
        Some("bool") => parse_bool_literal(num_part, span),
        Some("usize") => parse_uint_with_bits(num_part, 64, span),
        Some("isize") => parse_int_with_bits(num_part, 64, span),
        Some(suffix) if suffix.starts_with('u') => {
            let bits = parse_bits(&suffix[1..], span)?;
            parse_uint_with_bits(num_part, bits, span)
        }
        Some(suffix) if suffix.starts_with('i') => {
            let bits = parse_bits(&suffix[1..], span)?;
            parse_int_with_bits(num_part, bits, span)
        }
        Some(suffix) if suffix.starts_with('f') => {
            let bits = parse_bits(&suffix[1..], span)?;
            parse_float_with_bits(num_part, bits, span)
        }
        Some(other) => Err(type_error(
            format!("unknown numeric suffix '{other}'").as_str(),
            span,
        )),
        None => {
            if is_float {
                parse_float_with_bits(num_part, 64, span)
            } else {
                parse_int_with_bits(num_part, 64, span)
            }
        }
    }
}

fn parse_bool_literal(num_part: &str, span: Span) -> Result<Value, Diagnostic> {
    match num_part {
        "0" => Ok(Value::Bool(false)),
        "1" => Ok(Value::Bool(true)),
        _ => Err(type_error("bool literals must be 0bool or 1bool", span)),
    }
}

fn split_literal(literal: &str) -> (&str, Option<String>) {
    let mut split_index = None;
    for (idx, ch) in literal.char_indices() {
        if ch.is_ascii_alphabetic() {
            split_index = Some(idx);
            break;
        }
    }
    if let Some(idx) = split_index {
        (&literal[..idx], Some(literal[idx..].to_string()))
    } else {
        (literal, None)
    }
}

fn parse_bits(bits: &str, span: Span) -> Result<u16, Diagnostic> {
    let bits: u16 = bits
        .parse()
        .map_err(|_| Diagnostic::at_span("invalid numeric suffix", span))?;
    match bits {
        8 | 16 | 32 | 64 | 128 => Ok(bits),
        _ => Err(type_error("unsupported bit width", span)),
    }
}

fn parse_uint_with_bits(num_part: &str, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    if num_part.contains('.') {
        return Err(type_error("integer literal cannot contain '.'", span));
    }
    let value: u128 = num_part
        .parse()
        .map_err(|_| type_error("invalid integer literal", span))?;
    check_uint_range(value, bits, span)?;
    Ok(Value::UInt { bits, value })
}

fn parse_int_with_bits(num_part: &str, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    if num_part.contains('.') {
        return Err(type_error("integer literal cannot contain '.'", span));
    }
    let value: i128 = num_part
        .parse()
        .map_err(|_| type_error("invalid integer literal", span))?;
    check_int_range(value, bits, span)?;
    Ok(Value::Int { bits, value })
}

fn parse_float_with_bits(num_part: &str, bits: u16, span: Span) -> Result<Value, Diagnostic> {
    let value: f64 = num_part
        .parse()
        .map_err(|_| type_error("invalid float literal", span))?;
    if !value.is_finite() {
        return Err(type_error("float literal out of range", span));
    }
    match bits {
        32 => {
            let v32 = value as f32;
            if !v32.is_finite() {
                return Err(type_error("float literal out of range for f32", span));
            }
            Ok(Value::Float {
                bits,
                value: v32 as f64,
            })
        }
        64 => Ok(Value::Float { bits, value }),
        _ => Err(type_error("unsupported float width", span)),
    }
}

fn value_type(value: &Value) -> TypeName {
    match value {
        Value::Bool(_) => TypeName::Bool,
        Value::Str(_) => TypeName::String,
        Value::Char(_) => TypeName::Char,
        Value::Int { bits, .. } => TypeName::Int {
            signed: true,
            bits: *bits,
        },
        Value::UInt { bits, .. } => TypeName::Int {
            signed: false,
            bits: *bits,
        },
        Value::Float { bits, .. } => TypeName::Float { bits: *bits },
        Value::Ref { mutable, inner, .. } => TypeName::Ref {
            mutable: *mutable,
            inner: Box::new(inner.clone()),
        },
        Value::Struct { name, .. } => TypeName::Struct(name.clone()),
        Value::Enum {
            name, type_args, ..
        } => {
            if type_args.is_empty() {
                TypeName::Struct(name.clone())
            } else {
                TypeName::Applied {
                    name: name.clone(),
                    args: type_args
                        .iter()
                        .map(|arg| {
                            arg.clone()
                                .unwrap_or_else(|| TypeName::Struct("_".to_string()))
                        })
                        .collect(),
                }
            }
        }
        Value::Array { elem_type, elems } => TypeName::Array {
            elem: Box::new(elem_type.clone()),
            len: elems.len() as u64,
        },
        Value::List { elem_type, .. } => TypeName::List {
            elem: Box::new(elem_type.clone()),
        },
        Value::Dict {
            key_type,
            value_type,
            ..
        } => TypeName::Dict {
            key: Box::new(key_type.clone()),
            value: Box::new(value_type.clone()),
        },
        Value::Map {
            key_type,
            value_type,
            ..
        } => TypeName::Map {
            key: Box::new(key_type.clone()),
            value: Box::new(value_type.clone()),
        },
    }
}

fn coerce_value(value: Value, target: &TypeName, span: Span) -> Result<Value, Diagnostic> {
    match target {
        TypeName::Bool => match value {
            Value::Bool(value) => Ok(Value::Bool(value)),
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Char => match value {
            Value::Char(value) => Ok(Value::Char(value)),
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Byte => match value {
            Value::UInt { value, .. } => {
                check_uint_range(value, 8, span)?;
                Ok(Value::UInt { bits: 8, value })
            }
            Value::Int { value, .. } => {
                if value < 0 {
                    return Err(type_error("cannot coerce negative to byte", span));
                }
                let value =
                    u128::try_from(value).map_err(|_| type_error("cannot coerce to byte", span))?;
                check_uint_range(value, 8, span)?;
                Ok(Value::UInt { bits: 8, value })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::String => match value {
            Value::Str(text) => Ok(Value::Str(text)),
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Path => Err(type_error(
            "Path is an opaque runtime resource and must be constructed with Path::new",
            span,
        )),
        TypeName::File => Err(type_error(
            "File is an opaque runtime resource and cannot be constructed or coerced as a value",
            span,
        )),
        TypeName::Thread => Err(type_error(
            "Thread is an opaque linear owner and must be created with Thread::spawn",
            span,
        )),
        TypeName::Struct(name) => match value {
            Value::Struct {
                name: vname,
                fields,
            } => {
                if &vname != name {
                    return Err(type_mismatch(
                        target,
                        &Value::Struct {
                            name: vname,
                            fields,
                        },
                        span,
                    ));
                }
                Ok(Value::Struct {
                    name: vname,
                    fields,
                })
            }
            Value::Enum {
                name: enum_name,
                variant,
                type_args,
                payload,
            } => {
                if &enum_name != name || !type_args.is_empty() {
                    return Err(type_mismatch(
                        target,
                        &Value::Enum {
                            name: enum_name,
                            variant,
                            type_args,
                            payload,
                        },
                        span,
                    ));
                }
                Ok(Value::Enum {
                    name: name.clone(),
                    variant,
                    type_args,
                    payload,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Applied { name, args } => match value {
            Value::Enum {
                name: enum_name,
                variant,
                mut type_args,
                payload,
            } => {
                if &enum_name != name || type_args.len() != args.len() {
                    return Err(type_mismatch(
                        target,
                        &Value::Enum {
                            name: enum_name,
                            variant,
                            type_args,
                            payload,
                        },
                        span,
                    ));
                }
                for (inferred, target_arg) in type_args.iter_mut().zip(args.iter()) {
                    match inferred {
                        Some(source_arg) if source_arg != target_arg => {
                            return Err(type_mismatch(
                                target,
                                &Value::Enum {
                                    name: enum_name,
                                    variant,
                                    type_args,
                                    payload,
                                },
                                span,
                            ));
                        }
                        Some(_) => {}
                        slot @ None => *slot = Some(target_arg.clone()),
                    }
                }
                Ok(Value::Enum {
                    name: enum_name,
                    variant,
                    type_args,
                    payload,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Dict {
            key,
            value: target_value,
        } => match value {
            Value::Dict {
                key_type,
                value_type: source_value,
                entries,
            } => {
                let mut coerced_entries = BTreeMap::new();
                for (entry_key, entry_value) in entries {
                    let coerced_key = if key_type == **key {
                        entry_key
                    } else if key_type == TypeName::String {
                        dict_key_from_typed_value(Value::Str(entry_key), key.as_ref(), span)?
                    } else {
                        return Err(type_error("dictionary key type mismatch", span));
                    };
                    let coerced_entry = if source_value == **target_value {
                        entry_value
                    } else {
                        coerce_value(entry_value, target_value, span)?
                    };
                    coerced_entries.insert(coerced_key, coerced_entry);
                }
                Ok(Value::Dict {
                    key_type: (**key).clone(),
                    value_type: (**target_value).clone(),
                    entries: coerced_entries,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Map {
            key,
            value: target_value,
        } => match value {
            Value::Dict {
                value_type: source_value,
                entries,
                ..
            } => {
                let mut coerced_entries = BTreeMap::new();
                let mut coerced_keys = BTreeMap::new();
                for (entry_key, entry_value) in entries {
                    let source_key = Value::Str(entry_key);
                    let coerced_key = coerce_value(source_key, key.as_ref(), span)?;
                    let key_repr = dict_key_repr(&coerced_key, span)?;
                    let coerced_entry = if source_value == **target_value {
                        entry_value
                    } else {
                        coerce_value(entry_value, target_value, span)?
                    };
                    coerced_entries.insert(key_repr.clone(), coerced_entry);
                    coerced_keys.insert(key_repr, coerced_key);
                }
                Ok(Value::Map {
                    key_type: (**key).clone(),
                    value_type: (**target_value).clone(),
                    entries: coerced_entries,
                    keys: coerced_keys,
                })
            }
            Value::Map {
                key_type,
                value_type: source_value,
                entries,
                keys: source_keys,
            } => {
                let mut coerced_entries = BTreeMap::new();
                let mut coerced_keys = BTreeMap::new();
                for (entry_key, entry_value) in entries {
                    let source_key = source_keys
                        .get(&entry_key)
                        .cloned()
                        .ok_or_else(|| type_error("map key storage is inconsistent", span))?;
                    let coerced_key = if key_type == **key {
                        source_key
                    } else {
                        coerce_value(source_key, key.as_ref(), span)?
                    };
                    let key_repr = dict_key_repr(&coerced_key, span)?;
                    let coerced_entry = if source_value == **target_value {
                        entry_value
                    } else {
                        coerce_value(entry_value, target_value, span)?
                    };
                    coerced_entries.insert(key_repr.clone(), coerced_entry);
                    coerced_keys.insert(key_repr, coerced_key);
                }
                Ok(Value::Map {
                    key_type: (**key).clone(),
                    value_type: (**target_value).clone(),
                    entries: coerced_entries,
                    keys: coerced_keys,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Array { elem, len } => match value {
            Value::Array { elem_type, elems } => {
                if *len != elems.len() as u64 {
                    return Err(type_error("array length mismatch", span));
                }
                if !elems.is_empty() && &elem_type != elem.as_ref() {
                    return Err(type_error("array element type mismatch", span));
                }
                Ok(Value::Array {
                    elem_type: (**elem).clone(),
                    elems,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::List { elem } => match value {
            Value::Array { elems, .. } | Value::List { elems, .. } => {
                let mut coerced = Vec::with_capacity(elems.len());
                for value in elems {
                    coerced.push(coerce_value(value, elem, span)?);
                }
                Ok(Value::List {
                    elem_type: (**elem).clone(),
                    elems: coerced,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Float { bits } => match value {
            Value::Float {
                bits: value_bits,
                value,
            } => {
                if *bits == value_bits {
                    Ok(Value::Float { bits: *bits, value })
                } else if *bits == 64 && value_bits == 32 {
                    Ok(Value::Float { bits: 64, value })
                } else if *bits == 32 && value_bits == 64 {
                    let v32 = value as f32;
                    if !v32.is_finite() {
                        return Err(type_error("float out of range for f32", span));
                    }
                    Ok(Value::Float {
                        bits: 32,
                        value: v32 as f64,
                    })
                } else {
                    Err(type_mismatch(
                        target,
                        &Value::Float {
                            bits: value_bits,
                            value,
                        },
                        span,
                    ))
                }
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Int { signed, bits } => match value {
            Value::Int { value, .. } => {
                if *signed {
                    check_int_range(value, *bits, span)?;
                    Ok(Value::Int { bits: *bits, value })
                } else {
                    if value < 0 {
                        return Err(type_error("cannot coerce negative to unsigned", span));
                    }
                    let value = u128::try_from(value)
                        .map_err(|_| type_error("integer out of range", span))?;
                    check_uint_range(value, *bits, span)?;
                    Ok(Value::UInt { bits: *bits, value })
                }
            }
            Value::UInt { value, .. } => {
                if *signed {
                    let value = i128::try_from(value)
                        .map_err(|_| type_error("integer out of range", span))?;
                    check_int_range(value, *bits, span)?;
                    Ok(Value::Int { bits: *bits, value })
                } else {
                    check_uint_range(value, *bits, span)?;
                    Ok(Value::UInt { bits: *bits, value })
                }
            }
            other => Err(type_mismatch(target, &other, span)),
        },
        TypeName::Ref { mutable, inner } => match value {
            Value::Ref {
                target: target_name,
                mutable: value_mut,
                inner: value_inner,
            } => {
                if *mutable != value_mut || **inner != value_inner {
                    return Err(type_mismatch(
                        target,
                        &Value::Ref {
                            target: target_name.clone(),
                            mutable: value_mut,
                            inner: value_inner.clone(),
                        },
                        span,
                    ));
                }
                Ok(Value::Ref {
                    target: target_name,
                    mutable: value_mut,
                    inner: value_inner,
                })
            }
            other => Err(type_mismatch(target, &other, span)),
        },
    }
}

fn neg_value(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Int { bits, value } => {
            let value = value
                .checked_neg()
                .ok_or_else(|| type_error("integer overflow in negation", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        Value::Float { bits, value } => Ok(Value::Float {
            bits,
            value: -value,
        }),
        Value::UInt { .. } => Err(type_error("cannot negate unsigned integer", span)),
        Value::Str(_) => Err(type_error("cannot negate string", span)),
        Value::Char(_) => Err(type_error("cannot negate char", span)),
        Value::Bool(_) => Err(type_error("cannot negate bool", span)),
        Value::Ref { .. } => Err(type_error("cannot negate reference", span)),
        Value::Struct { .. } => Err(type_error("cannot negate struct", span)),
        Value::Enum { .. } => Err(type_error("cannot negate enum", span)),
        Value::Array { .. } => Err(type_error("cannot negate array", span)),
        Value::List { .. } => Err(type_error("cannot negate list", span)),
        Value::Dict { .. } => Err(type_error("cannot negate dictionary", span)),
        Value::Map { .. } => Err(type_error("cannot negate map", span)),
    }
}

fn not_value(value: Value, span: Span) -> Result<Value, Diagnostic> {
    match value {
        Value::Bool(value) => Ok(Value::Bool(!value)),
        _ => Err(type_error("logical not expects bool", span)),
    }
}

fn add_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Str(a), Value::Str(b)) => Ok(Value::Str(format!("{a}{b}"))),
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            let value = a
                .checked_add(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            let value = a
                .checked_add(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        (Value::Float { bits, value: a }, Value::Float { value: b, .. }) => {
            Ok(Value::Float { bits, value: a + b })
        }
        _ => Err(type_error("type mismatch in addition", span)),
    }
}

fn sub_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            let value = a
                .checked_sub(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            let value = a
                .checked_sub(b)
                .ok_or_else(|| type_error("integer underflow", span))?;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        (Value::Float { bits, value: a }, Value::Float { value: b, .. }) => {
            Ok(Value::Float { bits, value: a - b })
        }
        _ => Err(type_error("type mismatch in subtraction", span)),
    }
}

fn mul_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            let value = a
                .checked_mul(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            let value = a
                .checked_mul(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        (Value::Float { bits, value: a }, Value::Float { value: b, .. }) => {
            Ok(Value::Float { bits, value: a * b })
        }
        _ => Err(type_error("type mismatch in multiplication", span)),
    }
}

fn div_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            if b == 0 {
                return Err(type_error("division by zero", span));
            }
            let value = a
                .checked_div(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            if b == 0 {
                return Err(type_error("division by zero", span));
            }
            let value = a
                .checked_div(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        (Value::Float { bits, value: a }, Value::Float { value: b, .. }) => {
            if b == 0.0 {
                return Err(type_error("division by zero", span));
            }
            Ok(Value::Float { bits, value: a / b })
        }
        _ => Err(type_error("type mismatch in division", span)),
    }
}

fn mod_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            if b == 0 {
                return Err(type_error("division by zero", span));
            }
            let value = a
                .checked_rem(b)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            if b == 0 {
                return Err(type_error("division by zero", span));
            }
            let value = a % b;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        (Value::Float { bits, value: a }, Value::Float { value: b, .. }) => {
            if b == 0.0 {
                return Err(type_error("division by zero", span));
            }
            Ok(Value::Float { bits, value: a % b })
        }
        _ => Err(type_error("type mismatch in modulo", span)),
    }
}

fn bitand_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            let value = a & b;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            let value = a & b;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        _ => Err(type_error("type mismatch in bitwise and", span)),
    }
}

fn bitor_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            let value = a | b;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            let value = a | b;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        _ => Err(type_error("type mismatch in bitwise or", span)),
    }
}

fn bitxor_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    match (left, right) {
        (Value::Int { bits, value: a }, Value::Int { value: b, .. }) => {
            let value = a ^ b;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        (Value::UInt { bits, value: a }, Value::UInt { value: b, .. }) => {
            let value = a ^ b;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        _ => Err(type_error("type mismatch in bitwise xor", span)),
    }
}

fn shift_amount(value: Value, span: Span) -> Result<u32, Diagnostic> {
    match value {
        Value::UInt { value, .. } => Ok((value & 63) as u32),
        Value::Int { value, .. } => Ok(((value as u128) & 63) as u32),
        _ => Err(type_error("shift amount must be integer", span)),
    }
}

fn shl_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    let shift = shift_amount(right, span)?;
    match left {
        Value::Int { bits, value } => {
            let value = value
                .checked_shl(shift)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        Value::UInt { bits, value } => {
            let value = value
                .checked_shl(shift)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        _ => Err(type_error("left operand of shift must be integer", span)),
    }
}

fn shr_values(left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    let shift = shift_amount(right, span)?;
    match left {
        Value::Int { bits, value } => {
            let value = value
                .checked_shr(shift)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_int_range(value, bits, span)?;
            Ok(Value::Int { bits, value })
        }
        Value::UInt { bits, value } => {
            let value = value
                .checked_shr(shift)
                .ok_or_else(|| type_error("integer overflow", span))?;
            check_uint_range(value, bits, span)?;
            Ok(Value::UInt { bits, value })
        }
        _ => Err(type_error("left operand of shift must be integer", span)),
    }
}

fn value_partial_cmp(
    left: &Value,
    right: &Value,
    span: Span,
) -> Result<std::cmp::Ordering, Diagnostic> {
    match (left, right) {
        (Value::Int { value: a, .. }, Value::Int { value: b, .. }) => Ok(a.cmp(b)),
        (Value::UInt { value: a, .. }, Value::UInt { value: b, .. }) => Ok(a.cmp(b)),
        (Value::Float { value: a, .. }, Value::Float { value: b, .. }) => a
            .partial_cmp(b)
            .ok_or_else(|| type_error("float comparison with NaN is not supported", span)),
        (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b)),
        (Value::Char(a), Value::Char(b)) => Ok(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        (
            Value::Enum {
                name: left_name,
                variant: left_variant,
                type_args: left_type_args,
                payload: left_payload,
            },
            Value::Enum {
                name: right_name,
                variant: right_variant,
                type_args: right_type_args,
                payload: right_payload,
            },
        ) => {
            if left_name != right_name || left_type_args != right_type_args {
                return Err(type_error(
                    "cannot compare values from different enums",
                    span,
                ));
            }
            let variant_order = left_variant.cmp(right_variant);
            if variant_order != std::cmp::Ordering::Equal {
                return Ok(variant_order);
            }
            enum_payload_partial_cmp(left_payload, right_payload, span)
        }
        (
            Value::Struct {
                name: left_name,
                fields: left_fields,
            },
            Value::Struct {
                name: right_name,
                fields: right_fields,
            },
        ) => {
            if left_name != right_name {
                return Err(type_error(
                    "cannot compare values from different structs",
                    span,
                ));
            }
            let mut left_keys: Vec<&String> = left_fields.keys().collect();
            let mut right_keys: Vec<&String> = right_fields.keys().collect();
            left_keys.sort_unstable();
            right_keys.sort_unstable();
            if left_keys != right_keys {
                return Err(type_error(
                    "cannot compare structs with different field sets",
                    span,
                ));
            }
            for key in left_keys {
                let left_value = left_fields
                    .get(key)
                    .expect("left struct field key should exist");
                let right_value = right_fields
                    .get(key)
                    .expect("right struct field key should exist");
                let ord = value_partial_cmp(left_value, right_value, span)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(std::cmp::Ordering::Equal)
        }
        (
            Value::Array {
                elems: left_elems, ..
            },
            Value::Array {
                elems: right_elems, ..
            },
        ) => {
            let prefix_len = left_elems.len().min(right_elems.len());
            for idx in 0..prefix_len {
                let ord = value_partial_cmp(&left_elems[idx], &right_elems[idx], span)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(left_elems.len().cmp(&right_elems.len()))
        }
        (
            Value::List {
                elem_type: left_type,
                elems: left_elems,
            },
            Value::List {
                elem_type: right_type,
                elems: right_elems,
            },
        ) => {
            if left_type != right_type {
                return Err(type_error(
                    "cannot compare lists with different types",
                    span,
                ));
            }
            let prefix_len = left_elems.len().min(right_elems.len());
            for idx in 0..prefix_len {
                let ord = value_partial_cmp(&left_elems[idx], &right_elems[idx], span)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(left_elems.len().cmp(&right_elems.len()))
        }
        (
            Value::Dict {
                key_type: left_key_type,
                value_type: left_value_type,
                entries: left_entries,
            },
            Value::Dict {
                key_type: right_key_type,
                value_type: right_value_type,
                entries: right_entries,
            },
        ) => {
            if left_key_type != right_key_type || left_value_type != right_value_type {
                return Err(type_error(
                    "cannot compare dictionaries with different types",
                    span,
                ));
            }
            let mut left_iter = left_entries.iter();
            let mut right_iter = right_entries.iter();
            loop {
                match (left_iter.next(), right_iter.next()) {
                    (Some((lk, lv)), Some((rk, rv))) => {
                        let key_ord = lk.cmp(rk);
                        if key_ord != std::cmp::Ordering::Equal {
                            return Ok(key_ord);
                        }
                        let val_ord = value_partial_cmp(lv, rv, span)?;
                        if val_ord != std::cmp::Ordering::Equal {
                            return Ok(val_ord);
                        }
                    }
                    (None, None) => return Ok(std::cmp::Ordering::Equal),
                    (None, Some(_)) => return Ok(std::cmp::Ordering::Less),
                    (Some(_), None) => return Ok(std::cmp::Ordering::Greater),
                }
            }
        }
        (
            Value::Map {
                key_type: left_key_type,
                value_type: left_value_type,
                entries: left_entries,
                ..
            },
            Value::Map {
                key_type: right_key_type,
                value_type: right_value_type,
                entries: right_entries,
                ..
            },
        ) => {
            if left_key_type != right_key_type || left_value_type != right_value_type {
                return Err(type_error("cannot compare maps with different types", span));
            }
            let mut left_iter = left_entries.iter();
            let mut right_iter = right_entries.iter();
            loop {
                match (left_iter.next(), right_iter.next()) {
                    (Some((lk, lv)), Some((rk, rv))) => {
                        let key_ord = lk.cmp(rk);
                        if key_ord != std::cmp::Ordering::Equal {
                            return Ok(key_ord);
                        }
                        let val_ord = value_partial_cmp(lv, rv, span)?;
                        if val_ord != std::cmp::Ordering::Equal {
                            return Ok(val_ord);
                        }
                    }
                    (None, None) => return Ok(std::cmp::Ordering::Equal),
                    (None, Some(_)) => return Ok(std::cmp::Ordering::Less),
                    (Some(_), None) => return Ok(std::cmp::Ordering::Greater),
                }
            }
        }
        (
            Value::Ref {
                target: left_target,
                mutable: left_mutable,
                inner: left_inner,
            },
            Value::Ref {
                target: right_target,
                mutable: right_mutable,
                inner: right_inner,
            },
        ) => {
            let lhs = format!(
                "{}:{}:{}",
                if *left_mutable { "m" } else { "s" },
                left_target,
                left_inner.display()
            );
            let rhs = format!(
                "{}:{}:{}",
                if *right_mutable { "m" } else { "s" },
                right_target,
                right_inner.display()
            );
            Ok(lhs.cmp(&rhs))
        }
        _ => Err(type_error("type mismatch in comparison", span)),
    }
}

fn enum_payload_partial_cmp(
    left: &EnumPayloadValue,
    right: &EnumPayloadValue,
    span: Span,
) -> Result<std::cmp::Ordering, Diagnostic> {
    match (left, right) {
        (EnumPayloadValue::Unit, EnumPayloadValue::Unit) => Ok(std::cmp::Ordering::Equal),
        (EnumPayloadValue::Tuple(left), EnumPayloadValue::Tuple(right)) => {
            for (left, right) in left.iter().zip(right.iter()) {
                let ordering = value_partial_cmp(left, right, span)?;
                if ordering != std::cmp::Ordering::Equal {
                    return Ok(ordering);
                }
            }
            Ok(left.len().cmp(&right.len()))
        }
        (EnumPayloadValue::Named(left), EnumPayloadValue::Named(right)) => {
            let mut left_names: Vec<&String> = left.keys().collect();
            let mut right_names: Vec<&String> = right.keys().collect();
            left_names.sort_unstable();
            right_names.sort_unstable();
            let name_order = left_names.cmp(&right_names);
            if name_order != std::cmp::Ordering::Equal {
                return Ok(name_order);
            }
            for name in left_names {
                let ordering = value_partial_cmp(
                    left.get(name).expect("enum payload field should exist"),
                    right.get(name).expect("enum payload field should exist"),
                    span,
                )?;
                if ordering != std::cmp::Ordering::Equal {
                    return Ok(ordering);
                }
            }
            Ok(std::cmp::Ordering::Equal)
        }
        _ => Err(type_error("enum payload shape mismatch", span)),
    }
}

fn cmp_values(op: BinaryOp, left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    let ordering = value_partial_cmp(&left, &right, span)?;
    let out = match op {
        BinaryOp::Eq => ordering == std::cmp::Ordering::Equal,
        BinaryOp::Ne => ordering != std::cmp::Ordering::Equal,
        BinaryOp::Lt => ordering == std::cmp::Ordering::Less,
        BinaryOp::Le => ordering != std::cmp::Ordering::Greater,
        BinaryOp::Gt => ordering == std::cmp::Ordering::Greater,
        BinaryOp::Ge => ordering != std::cmp::Ordering::Less,
        _ => return Err(type_error("invalid comparison", span)),
    };
    Ok(Value::Bool(out))
}

fn check_int_range(value: i128, bits: u16, span: Span) -> Result<(), Diagnostic> {
    if bits == 128 {
        return Ok(());
    }
    let max = (1_i128 << (bits - 1)) - 1;
    let min = -(1_i128 << (bits - 1));
    if value < min || value > max {
        Err(type_error("integer out of range", span))
    } else {
        Ok(())
    }
}

fn check_uint_range(value: u128, bits: u16, span: Span) -> Result<(), Diagnostic> {
    if bits == 128 {
        return Ok(());
    }
    let max = (1_u128 << bits) - 1;
    if value > max {
        Err(type_error("integer out of range", span))
    } else {
        Ok(())
    }
}

fn type_error(message: &str, span: Span) -> Diagnostic {
    Diagnostic::at_span(message, span)
}

fn type_mismatch(expected: &TypeName, got: &Value, span: Span) -> Diagnostic {
    let got_type = value_type(got).display();
    let expected = expected.display();
    Diagnostic::at_span(
        format!("type mismatch: expected {expected}, got {got_type}"),
        span,
    )
}
