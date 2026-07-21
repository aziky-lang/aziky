use super::lower_program;
use crate::frontend::lexer::lex;
use crate::frontend::parser::parse;

#[cfg(any())]
#[test]
fn lower_prefix_scan_to_vectorizable_terminal_sum_kernel() {
    let src = include_str!("../../../bench/prefix_scan.azk");
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimePrefixScanLoop {
            batches: 1_000_000,
            state_init: 123_456_789,
            mul: 1_664_525,
            add: 1_013_904_223,
            state_mask: 0xffff_ffff,
            value_mask: 0xffff,
            width: 16,
            exit_mask: 127,
        })
    ));
}

#[test]
fn lower_program_with_function_call() {
    let src = "fn helper() { print(\"hi\"); } fn main() { helper(); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_expression_function_call_with_return() {
    let src = "fn add1(x: i32) -> i32 { return x + 1i32; } fn main() { let y: i32 = add1(41i32); print(y.to_str()); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(lowered.first(), Some(super::LoweredStmt::Print(text)) if text == "42"));
    assert!(matches!(lowered.last(), Some(super::LoweredStmt::Exit(0))));
}

#[test]
fn lower_expression_method_call_with_args() {
    let src = "struct P { x: i32; } fn shift(self_ref: &P, d: i32) -> i32 { return self_ref.x + d; } fn main() { let p: P = P { x: 1i32 }; let y: i32 = p.shift(2i32); print(y.to_str()); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(lowered.first(), Some(super::LoweredStmt::Print(text)) if text == "3"));
    assert!(matches!(lowered.last(), Some(super::LoweredStmt::Exit(0))));
}

#[test]
fn lower_expression_function_call_with_pure_local_mutation() {
    let src = "fn sum_to(n: i32) -> i32 { let mut i: i32 = 0i32; let mut s: i32 = 0i32; while i < n { s = s + i; i = i + 1i32; } return s; } fn main() { let y: i32 = sum_to(4i32); print(y.to_str()); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(lowered.first(), Some(super::LoweredStmt::Print(text)) if text == "6"));
    assert!(matches!(lowered.last(), Some(super::LoweredStmt::Exit(0))));
}

#[test]
fn lower_expression_function_call_rejects_side_effects() {
    let src = "fn noisy(x: i32) -> i32 { print(\"x\"); return x; } fn main() { let y: i32 = noisy(1i32); print(y.to_str()); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected side-effect expression call error");
    assert!(err.message.contains("requires pure function"));
}

#[test]
fn lower_runtime_generic_call_with_args() {
    let src = "fn helper(x: u64, y: u64) { let z: u64 = x + y; } fn main() { let mut s: u64 = runtime_seed(); helper(s, 7u64); s = s + 1u64; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let Some(super::LoweredStmt::RuntimeGeneric { program }) = lowered.first() else {
        panic!("expected runtime generic lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }))
    );
}

#[test]
fn lower_runtime_generic_bool_return_from_comparison() {
    let src = "fn is_separator(value: char) -> bool { return value == '/'; } fn main() { let seed: u64 = runtime_seed(); let valid: bool = is_separator('/'); if valid { exit(seed & 0u64); } exit(1u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("comparison return must lower natively");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_runtime_generic_without_runtime_seed_when_supported() {
    let src = "fn main() { let a: u64 = 1u64; let b: u64 = 2u64; let c: u64 = a + b; exit(c); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_runtime_generic_falls_back_without_runtime_intrinsics() {
    let src = "fn main() { let s = \"42\"; print(s); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::Print(v)) if v == "42"
    ));
}

#[test]
fn lower_runtime_generic_assert_emits_failure_exit() {
    let src = "fn main() { let s: u64 = runtime_seed(); assert(s == s, \"ok\"); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let Some(super::LoweredStmt::RuntimeGeneric { program }) = lowered.first() else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Exit {
                code: super::RuntimeOperand::Imm(1)
            }
        )
    }));
}

#[test]
fn lower_runtime_generic_panic_emits_exit_101() {
    let src =
        "fn main() { let s: u64 = runtime_seed(); if s == 0u64 { panic(\"boom\"); } exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let Some(super::LoweredStmt::RuntimeGeneric { program }) = lowered.first() else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Exit {
                code: super::RuntimeOperand::Imm(101)
            }
        )
    }));
}

#[test]
fn lower_nested_call_error_contains_stack_trace() {
    let src = "fn inner(x: i32) { let b: u8 = x; } fn middle() { inner(300i32); } fn main() { middle(); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected nested-call diagnostic");
    assert!(
        err.trace
            .iter()
            .any(|frame| frame.contains("function 'inner'"))
    );
    assert!(
        err.trace
            .iter()
            .any(|frame| frame.contains("function 'middle'"))
    );
    assert!(
        err.trace
            .iter()
            .any(|frame| frame.contains("function 'main'"))
    );
}

#[test]
fn lower_struct_and_array() {
    let src = "struct P { x: i32; } fn main() { let p = P { x: 1i32 }; let a: [u8; 2] = [1u8, 2u8]; print(p.x); print(a[1u8]); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_struct_field_assignment() {
    let src = "struct P { x: u64; } fn main() { let mut p: P = P { x: 1u64 }; p.x = 7u64; print(p.x.to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::Print(v)) if v == "7"
    ));
}

#[test]
fn lower_with_conversions() {
    let src = "fn main() { let s = \"42\"; let n: i32 = s.to_i32(); let b: bool = \"true\".to_bool(); let f: f32 = n.to_f32(); let t = n.to_str(); print(t); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert_eq!(lowered.len(), 2);
}

#[test]
fn lower_extended_loops() {
    let src = "fn main() { let mut c: u8 = 0u8; for i in 0u8..5u8 { if i == 3u8 { break; } c = c + 1u8; } loop { continue; } exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_std_methods() {
    let src = "fn main() { let s = \"  HeLLo \"; let trimmed = s.trim(); print(trimmed.to_lower()); print(\"\\n\"); let arr: [u8; 3] = [1u8, 2u8, 3u8]; print(arr.len()); print(\"\\n\"); let n: i32 = (-3i32).abs(); print(n.to_str()); print(\"\\n\"); print((1u8).is_positive().to_str()); print(\"\\n\"); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(lowered.len() >= 5);
}

#[test]
fn lower_char_literals_unicode_methods_collections_and_ordering() {
    let src = "fn main() { let letter: char = 'λ'; print(letter); print(\"\\n\"); print(letter.is_alphabetic().to_str()); print(\"\\n\"); print(letter.type_name()); print(\"\\n\"); let sharp: char = 'ß'; print(sharp.to_upper()); print(\"\\n\"); let ascii: char = 'q'.to_ascii_upper(); print(ascii); print(\"\\n\"); let code: u32 = letter.to_u32(); print(code); print(\"\\n\"); let mut lookup: dict<char, u64> = {}; lookup.set(letter, 9u64); print(lookup[letter]); print(\"\\n\"); let mut chars: [char; 3] = ['z', 'a', 'm']; chars.sort(); print(chars[0u8]); print(\"\\n\"); print((letter > 'A').to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec![
            "λ", "\n", "true", "\n", "char", "\n", "SS", "\n", "Q", "\n", "955", "\n", "9", "\n",
            "a", "\n", "true"
        ]
    );
}

#[test]
fn char_rejects_ambiguous_integer_conversions() {
    let tokens = lex("fn main() { let code: u8 = 'A'.to_u8(); exit(0u64); }").expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected explicit char conversion error");
    assert!(
        err.message
            .contains("char only converts explicitly with to_u32()")
    );
}

#[test]
fn lower_typed_std_methods_with_arguments() {
    let src = "fn main() { let text: string = \"λambda\"; print(text.contains('λ').to_str()); print(\"\\n\"); print(text.starts_with(\"λam\").to_str()); print(\"\\n\"); print(text.ends_with(\"da\").to_str()); print(\"\\n\"); print(text.replace('λ', \"L\")); print(\"\\n\"); print(\"ab\".repeat(3u8)); print(\"\\n\"); print(\"λx\".char_count()); print(\"\\n\"); print(\"λx\".len()); print(\"\\n\"); let values: [i32; 3] = [2i32, 4i32, 6i32]; print(values.contains(4i32).to_str()); print(\"\\n\"); let mut scores: dict<string, i32> = {}; scores.set(\"aziky\", 9i32); print(scores.contains_key(\"aziky\").to_str()); print(\"\\n\"); print((12i32).min(8i32)); print(\"\\n\"); print((12i32).max(20i32)); print(\"\\n\"); print((12i32).clamp(0i32, 10i32)); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec![
            "true", "\n", "true", "\n", "true", "\n", "Lambda", "\n", "ababab", "\n", "2", "\n",
            "3", "\n", "true", "\n", "true", "\n", "8", "\n", "20", "\n", "10"
        ]
    );
}

#[test]
fn std_method_arity_and_type_errors_are_specific() {
    let tokens = lex("fn main() { let text = \"aziky\"; print(text.contains()); exit(0u64); }")
        .expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected method arity error");
    assert!(
        err.message
            .contains("method 'contains' expects 1 argument, got 0")
    );

    let tokens =
        lex("fn main() { print(\"aziky\".repeat(true)); exit(0u64); }").expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected repeat type error");
    assert!(err.message.contains("expects an integer count, got bool"));

    let tokens =
        lex("fn main() { print((5i32).clamp(10i32, 1i32)); exit(0u64); }").expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected clamp bounds error");
    assert!(
        err.message
            .contains("minimum must not be greater than maximum")
    );
}

#[test]
fn lower_enum_foreach_and_dict() {
    let src = "enum State { Ready, Busy } fn main() { let s: State = State::Ready; let d: dict<string, i32> = {\"a\": 1i32, \"b\": 2i32}; foreach k in d { print(k); } let values = d.keys(); foreach v in values { print(v); } if s == State::Ready { print(\"ok\"); } else if s == State::Busy { print(\"busy\"); } else { print(\"other\"); } print(d[\"a\"].to_str()); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(lowered.len() >= 5);
}

#[test]
fn lower_enum_payload_construction_and_formatting() {
    let src = "enum Message { Quit, Move(i32, i32), Write { text: string; code: u32; }, } fn moved(x: i32, y: i32) -> Message { return Message::Move(x, y); } fn main() { let first: Message = moved(3i32, 4i32); let second: Message = Message::Write { text: \"ready\", code: 7u32 }; print(first.to_str()); print(\"\\n\"); print(second.to_str()); print(\"\\n\"); print((first == Message::Move(3i32, 4i32)).to_str()); print(\"\\n\"); print((first.hash64() == Message::Move(3i32, 4i32).hash64()).to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec![
            "Message::Move(3, 4)",
            "\n",
            "Message::Write { code: 7, text: \"ready\" }",
            "\n",
            "true",
            "\n",
            "true"
        ]
    );
}

#[test]
fn lower_exhaustive_match_destructures_every_payload_shape() {
    let src = "enum Message { Quit, Move(i32, i32), Write { text: string; code: u32; }, } fn describe(message: Message) -> string { return match message { Message::Quit => \"quit\", Message::Move(x, y) => x.to_str() + \",\" + y.to_str(), Message::Write { text: body } => body, }; } fn main() { print(describe(Message::Quit)); print(\"\\n\"); print(describe(Message::Move(3i32, 4i32))); print(\"\\n\"); print(describe(Message::Write { text: \"ready\", code: 7u32 })); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["quit", "\n", "3,4", "\n", "ready"]);
}

#[test]
fn lower_match_wildcard_is_scoped_and_catches_remaining_variants() {
    let src = "enum State { Ready, Busy, Done, } fn label(state: State) -> string { return match state { State::Ready => \"ready\", _ => \"other\", }; } fn main() { print(label(State::Busy)); print(label(State::Done)); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["other", "other"]);
}

#[test]
fn match_rejects_non_exhaustive_and_unreachable_arms() {
    let cases = [
        (
            "enum State { Ready, Busy, Done, } fn main() { let state: State = State::Ready; let label: string = match state { State::Ready => \"ready\", State::Busy => \"busy\", }; exit(0u64); }",
            "non-exhaustive match",
        ),
        (
            "enum State { Ready, Busy, } fn main() { let state: State = State::Ready; let label: string = match state { State::Ready => \"a\", State::Ready => \"b\", State::Busy => \"c\", }; exit(0u64); }",
            "unreachable duplicate match arm",
        ),
        (
            "enum State { Ready, Busy, } fn main() { let state: State = State::Ready; let label: string = match state { _ => \"any\", State::Ready => \"ready\", }; exit(0u64); }",
            "wildcard match arm must appear exactly once and be last",
        ),
        (
            "enum State { Ready, Busy, } fn main() { let state: State = State::Ready; let label: string = match state { State::Ready => \"a\", State::Busy => \"b\", _ => \"c\", }; exit(0u64); }",
            "unreachable wildcard match arm",
        ),
    ];
    for (src, expected) in cases {
        let tokens = lex(src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let err = lower_program(&program).expect_err("expected invalid match error");
        assert!(
            err.message.contains(expected),
            "expected {expected:?}, got {:?}",
            err.message
        );
    }
}

#[test]
fn match_validates_payload_shapes_fields_and_bindings() {
    let cases = [
        (
            "enum Message { Move(i32, i32), } fn main() { let value: Message = Message::Move(1i32, 2i32); let text: string = match value { Message::Move(x) => x.to_str(), }; exit(0u64); }",
            "expects 2 fields, got 1",
        ),
        (
            "enum Message { Move(i32, i32), } fn main() { let value: Message = Message::Move(1i32, 2i32); let text: string = match value { Message::Move(x, x) => x.to_str(), }; exit(0u64); }",
            "duplicate binding 'x'",
        ),
        (
            "enum Message { Write { text: string; }, } fn main() { let value: Message = Message::Write { text: \"ok\" }; let text: string = match value { Message::Write { missing } => missing, }; exit(0u64); }",
            "unknown payload field 'missing'",
        ),
        (
            "enum Message { Quit, } fn main() { let value: Message = Message::Quit; let text: string = match value { Message::Quit(x) => x.to_str(), }; exit(0u64); }",
            "uses tuple syntax for a non-tuple variant",
        ),
    ];
    for (src, expected) in cases {
        let tokens = lex(src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let err = lower_program(&program).expect_err("expected match pattern error");
        assert!(
            err.message.contains(expected),
            "expected {expected:?}, got {:?}",
            err.message
        );
    }
}

#[test]
fn generic_enums_infer_payload_types_and_match_concretely() {
    let src = "enum Outcome<T, E> { Ok(T), Err(E), } fn render(value: Outcome<i32, string>) -> string { return match value { Outcome::Ok(number) => number.to_str(), Outcome::Err(message) => message, }; } fn main() { let ok: Outcome<i32, string> = Outcome::Ok(7i32); let err: Outcome<i32, string> = Outcome::Err(\"failed\"); print(render(ok)); print(\"\\n\"); print(render(err)); print(\"\\n\"); print(ok.type_name()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec!["7", "\n", "failed", "\n", "Outcome<i32, string>"]
    );
}

#[test]
fn builtin_option_and_result_are_typed_exhaustive_enums() {
    let src = "fn maybe(enabled: bool) -> Option<i32> { if enabled { return Option::Some(9i32); } return Option::None; } fn divide(ok: bool) -> Result<i32, string> { if ok { return Result::Ok(12i32); } return Result::Err(\"division failed\"); } fn option_text(value: Option<i32>) -> string { return match value { Option::Some(number) => number.to_str(), Option::None => \"none\", }; } fn result_text(value: Result<i32, string>) -> string { return match value { Result::Ok(number) => number.to_str(), Result::Err(message) => message, }; } fn main() { print(option_text(maybe(true))); print(\"\\n\"); print(option_text(maybe(false))); print(\"\\n\"); print(result_text(divide(true))); print(\"\\n\"); print(result_text(divide(false))); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec!["9", "\n", "none", "\n", "12", "\n", "division failed"]
    );
}

#[test]
fn safe_lookup_parsing_and_unicode_scalar_apis_return_values() {
    let src = "fn main() { let values: [i32; 2] = [4i32, 8i32]; let found: Option<i32> = values.get(1u8); let absent: Option<i32> = values.get(9u8); let words: dict<string, i32> = {\"aziky\": 7i32}; let known: Option<i32> = words.get(\"aziky\"); let missing: Option<i32> = words.get(\"missing\"); let letter: Option<char> = \"λx\".char_at(0u8); let beyond: Option<char> = \"λx\".char_at(4u8); let parsed: Result<i32, string> = \"42\".parse_i32(); let invalid: Result<i32, string> = \"forty-two\".parse_i32(); let scalar: Option<char> = (955u32).to_char_checked(); let surrogate: Option<char> = (55296u32).to_char_checked(); print(found.unwrap_or(0i32).to_str()); print(absent.unwrap_or(3i32).to_str()); print(known.unwrap_or(0i32).to_str()); print(missing.is_none().to_str()); print(letter.unwrap_or('?')); print(beyond.is_none().to_str()); print(parsed.unwrap_or(0i32).to_str()); print(invalid.is_err().to_str()); print(scalar.unwrap_or('?')); print(surrogate.is_none().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prints,
        vec![
            "8", "3", "7", "true", "λ", "true", "42", "true", "λ", "true"
        ]
    );
}

#[test]
fn checked_parse_family_preserves_failures_without_panicking() {
    let src = "fn main() { let a: Result<u8, string> = \"255\".parse_u8(); let b: Result<u8, string> = \"256\".parse_u8(); let c: Result<f64, string> = \"3.5\".parse_f64(); let d: Result<bool, string> = \"true\".parse_bool(); let e: Result<bool, string> = \"yes\".parse_bool(); print(a.is_ok().to_str()); print(b.is_err().to_str()); print(c.unwrap_or(0.0f64).to_str()); print(d.unwrap_or(false).to_str()); print(e.is_err().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["true", "true", "3.5", "true", "true"]);
}

#[test]
fn owned_lists_and_maps_have_dynamic_mutation_and_safe_access() {
    let src = "fn sum(values: list<i32>) -> i32 { let mut total: i32 = 0i32; foreach value in values { total = total + value; } return total; } fn main() { let mut values: list<i32> = []; values.reserve(8u8); values.push(2i32); values.push(4i32); values.push(8i32); values[1u8] = 5i32; let tail: Option<i32> = values.peek(); values.pop(); values.shrink_to(4u8); values.shrink_to_fit(); let mut scores: map<string, i32> = {}; scores.set(\"aziky\", 7i32); scores[\"compiler\"] = 9i32; let known: Option<i32> = scores.get(\"compiler\"); scores.remove(\"aziky\"); print(sum(values).to_str()); print(\"\\n\"); print(tail.unwrap_or(0i32).to_str()); print(\"\\n\"); print(known.unwrap_or(0i32).to_str()); print(\"\\n\"); print(scores.len().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["7", "\n", "8", "\n", "9", "\n", "1"]);
}

#[test]
fn owned_list_first_and_last_preserve_empty_state_as_option() {
    let src = "fn main() { let populated: list<i32> = [3i32, 7i32]; let empty: list<i32> = []; print(populated.first().unwrap_or(0i32).to_str()); print(populated.last().unwrap_or(0i32).to_str()); print(empty.first().is_none().to_str()); print(empty.last().is_none().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["3", "7", "true", "true"]);
}

#[test]
fn owned_map_preserves_typed_keys_for_iteration_and_keys() {
    let src = "fn main() { let mut values: map<u32, string> = {}; values.set(2u32, \"two\"); values[7u32] = \"seven\"; let mut total: u32 = 0u32; foreach key in values { total = total + key; } let keys = values.keys(); print(total.to_str()); print(\"\\n\"); print(keys[0u8].to_str()); print(\"\\n\"); print(keys[1u8].to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["9", "\n", "2", "\n", "7"]);
}

#[test]
fn owned_collections_compare_by_typed_contents() {
    let src = "fn main() { let left: list<i32> = [1i32, 2i32]; let right: list<i32> = [1i32, 2i32]; let mut first: map<u32, string> = {}; first.set(3u32, \"three\"); let mut second: map<u32, string> = {}; second.set(3u32, \"three\"); print((left == right).to_str()); print((first == second).to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let prints: Vec<&str> = lowered
        .iter()
        .filter_map(|stmt| match stmt {
            super::LoweredStmt::Print(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prints, vec!["true", "true"]);
}

#[test]
fn untyped_empty_collection_requires_an_annotation() {
    let tokens = lex("fn main() { let values = []; exit(0u64); }").expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("empty collection must need a type");
    assert!(
        err.message
            .contains("cannot infer every generic type argument")
    );
}

#[test]
fn generic_enum_diagnostics_reject_ambiguity_and_conflicts() {
    let cases = [
        (
            "fn main() { let missing = Option::None; exit(0u64); }",
            "cannot infer every generic type argument",
        ),
        (
            "enum Same<T> { Pair(T, T), } fn main() { let value: Same<i32> = Same::Pair(1i32, 2u32); exit(0u64); }",
            "conflicting inferred types for generic parameter 'T'",
        ),
        (
            "fn main() { let value: Option<i32, string> = Option::None; exit(0u64); }",
            "expects 1 type argument, got 2",
        ),
        (
            "fn consume(value: Option) -> i32 { return 0i32; } fn main() { exit(0u64); }",
            "requires 1 type argument",
        ),
        (
            "enum Bad<T, T> { Value(T), } fn main() { exit(0u64); }",
            "duplicate generic parameter 'T'",
        ),
    ];
    for (src, expected) in cases {
        let tokens = lex(src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let err = lower_program(&program).expect_err("expected generic enum error");
        assert!(
            err.message.contains(expected),
            "expected {expected:?}, got {:?}",
            err.message
        );
    }
}

#[test]
fn enum_payload_diagnostics_are_specific() {
    let cases = [
        (
            "enum Message { Move(i32, i32), } fn main() { let value: Message = Message::Move(1i32); exit(0u64); }",
            "expects 2 arguments, got 1",
        ),
        (
            "enum Message { Move(i32), } fn main() { let value: Message = Message::Move(\"bad\"); exit(0u64); }",
            "expected i32, got string",
        ),
        (
            "enum Message { Write { text: string; code: u32; }, } fn main() { let value: Message = Message::Write { text: \"ok\" }; exit(0u64); }",
            "missing payload field 'code'",
        ),
        (
            "enum Message { Write { text: string; }, } fn main() { let value: Message = Message::Write { text: \"ok\", code: 7u32 }; exit(0u64); }",
            "unknown payload field 'code'",
        ),
        (
            "enum Message { Quit, } fn main() { let value: Message = Message::Quit(1u8); exit(0u64); }",
            "expects no payload",
        ),
        (
            "enum Message { Write { text: string; }, } fn main() { let value: Message = Message::Write(); exit(0u64); }",
            "must be constructed with '{ ... }'",
        ),
    ];

    for (src, expected) in cases {
        let tokens = lex(src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let err = lower_program(&program).expect_err("expected enum payload error");
        assert!(
            err.message.contains(expected),
            "expected {expected:?}, got {:?}",
            err.message
        );
    }
}

#[test]
fn enum_payload_definitions_reject_ambiguous_shapes() {
    let cases = [
        (
            "enum Empty { Value(), } fn main() { exit(0u64); }",
            "must contain at least one field",
        ),
        (
            "enum Empty { Value {}, } fn main() { exit(0u64); }",
            "must contain at least one field",
        ),
        (
            "enum Bad { Value { code: u32; code: u64; }, } fn main() { exit(0u64); }",
            "duplicate payload field 'code'",
        ),
    ];

    for (src, expected) in cases {
        let tokens = lex(src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let err = lower_program(&program).expect_err("expected enum definition error");
        assert!(
            err.message.contains(expected),
            "expected {expected:?}, got {:?}",
            err.message
        );
    }
}

#[test]
fn lower_assert_and_panic_errors() {
    let src_assert = "fn main() { let msg = \"boom\"; assert(false, msg); exit(0); }";
    let tokens = lex(src_assert).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected assertion failure");
    assert!(err.message.contains("assertion failed"));

    let src_panic = "fn main() { let msg = \"stop\"; panic(msg); exit(0); }";
    let tokens = lex(src_panic).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected panic failure");
    assert!(err.message.contains("panic:"));
}

#[test]
fn lower_parallel_for_deterministic_order() {
    let src = "fn main() { parfor i in 0u8..4u8 { print(i.to_str()); print(\"\\n\"); } exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(lowered.first(), Some(super::LoweredStmt::Print(v)) if v == "0"));
}

#[test]
fn lower_parfor_reductions() {
    let src = "fn main() { let mut total: i64 = 0i64; let mut lo: i64 = 0i64; let mut hi: i64 = 0i64; parfor i in 1i64..5i64 reduce sum into total { i }; parfor i in -2i64..3i64 reduce min into lo { i }; parfor i in -2i64..3i64 reduce max into hi { i }; print(total.to_str()); print(\"\\n\"); print(lo.to_str()); print(\"\\n\"); print(hi.to_str()); print(\"\\n\"); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "10"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "-2"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "2"))
    );
}

#[test]
fn lower_parfor_reduction_strict_types() {
    let src = "fn main() { let mut total: f64 = 0.0f64; parfor i in 0u8..4u8 reduce sum into total { 1.0f64 }; exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected reduction type failure");
    assert!(err.message.contains("integer targets"));
}

#[cfg(any())]
#[test]
fn lower_runtime_benchloop() {
    let src = "fn main() { benchloop(1000000u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeBenchLoop { iterations }) if *iterations == 1_000_000
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_while_lcg_kernel() {
    let src = "fn main() { let mut state: u64 = 1u64; let mut i: u64 = 0u64; while i < 1000u64 { state = state * 1664525u64 + 1013904223u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeLcgLoop {
            iterations,
            state_init,
            mul,
            add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *state_init == 1 && *mul == 1_664_525 && *add == 1_013_904_223 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_for_lcg_kernel() {
    let src = "fn main() { let mut state: u64 = 7u64; for i in 10u64..1010u64 { state = state * 1664525u64 + 1013904223u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeLcgLoop {
            iterations,
            state_init,
            mul,
            add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *state_init == 7 && *mul == 1_664_525 && *add == 1_013_904_223 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_while_lcg_kernel() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 1000u64 { state = state * 1664525u64 + 1013904223u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededLcgLoop {
            iterations,
            mul,
            add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *mul == 1_664_525 && *add == 1_013_904_223 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_for_lcg_kernel() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); for i in 10u64..1010u64 { state = state * 1664525u64 + 1013904223u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededLcgLoop {
            iterations,
            mul,
            add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *mul == 1_664_525 && *add == 1_013_904_223 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_predictable_branch_kernel() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 1000u64 { if i < 250u64 { state = state * 1664525u64 + 1013904223u64; } else { state = state * 22695477u64 + 1u64; } i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededPredictableBranchLcgLoop {
            iterations,
            then_iterations,
            then_mul,
            then_add,
            else_mul,
            else_add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *then_iterations == 250 && *then_mul == 1_664_525 && *then_add == 1_013_904_223 && *else_mul == 22_695_477 && *else_add == 1 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_unpredictable_branch_kernel() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 1000u64 { if state < 9223372036854775808u64 { state = state * 1664525u64 + 1013904223u64; } else { state = state * 22695477u64 + 1u64; } i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop {
            iterations,
            threshold,
            then_mul,
            then_add,
            else_mul,
            else_add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *threshold == (1u64 << 63) && *then_mul == 1_664_525 && *then_add == 1_013_904_223 && *else_mul == 22_695_477 && *else_add == 1 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_masked_unpredictable_branch_kernel() {
    let src = "fn main() { let mut state: u64 = 123456789u64; let mut i: u64 = 0u64; while i < 1000u64 { if state < 2147483648u64 { state = (state * 1664525u64 + 1013904223u64) & 4294967295u64; } else { state = (state * 22695477u64 + 1u64) & 4294967295u64; } i = i + 1u64; } exit(state & 127u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeBranchLcgLoop {
            iterations: 1_000,
            state_init: 123_456_789,
            state_mask: 0xFFFF_FFFF,
            threshold: 2_147_483_648,
            ..
        })
    ));
}

#[test]
fn lower_runtime_generic_control_flow() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 64u64 { if i < 32u64 { state = state + i; } else { state = state + 2u64; } i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_runtime_generic_with_helper_call() {
    let src = "fn step() { let mut x: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 16u64 { x = x + 1u64; i = i + 1u64; } exit(x); } fn main() { step(); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_runtime_generic_emits_call_and_return() {
    let src = "fn helper() { let mut i: u64 = 0u64; while i < 4u64 { i = i + 1u64; } } fn main() { let mut state: u64 = runtime_seed(); helper(); state = state + 1u64; exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Return))
    );
}

#[test]
fn lower_runtime_generic_expr_call_with_return_value() {
    let src = "fn mix(x: u64, y: u64) -> u64 { return x * 3u64 + y; } fn main() { let mut state: u64 = runtime_seed(); state = mix(state, 7u64); exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Return))
    );
}

#[test]
fn lower_runtime_generic_expr_call_with_f64_arithmetic() {
    let src = "fn blend(x: f64, y: f64) -> f64 { return x * 1.5f64 + y; } fn main() { let mut state: u64 = runtime_seed(); let mut v: f64 = 2.0f64; v = blend(v, 3.0f64); state = state + 1u64; exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FloatBinOp { bits: 64, .. }))
    );
}

#[test]
fn lower_runtime_generic_expr_call_with_f32_arithmetic() {
    let src = "fn blend32(x: f32, y: f32) -> f32 { return x * 1.5f32 + y; } fn main() { let mut state: u64 = runtime_seed(); let mut v: f32 = 2.0f32; v = blend32(v, 3.0f32); state = state + 1u64; exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FloatBinOp { bits: 32, .. }))
    );
}

#[test]
fn lower_runtime_generic_return_requires_value() {
    let src = "fn mix(x: u64) -> u64 { return; } fn main() { let mut state: u64 = runtime_seed(); state = mix(state); exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected missing return value error");
    assert!(err.message.contains("return value is required"));
}

#[test]
fn lower_runtime_generic_recursive_call_rejected() {
    let src = "fn a() { let mut x: u64 = runtime_seed(); b(); exit(x); } fn b() { a(); } fn main() { a(); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected recursive call error");
    assert!(err.message.contains("recursive"));
}

#[test]
fn lower_runtime_generic_signed_cmp_control_flow() {
    let src = "fn main() { let mut state: i64 = runtime_seed(); let mut i: i64 = -8i64; while i < 8i64 { if i < 0i64 { state = state - i; } else { state = state + i; } i = i + 1i64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_signed_cmp = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::LtSigned,
                ..
            }
        )
    });
    assert!(has_signed_cmp);
}

#[test]
fn lower_runtime_generic_u32_emits_normalize_instr() {
    let src = "fn main() { let mut state: u32 = runtime_seed(); let mut i: u32 = 0u32; while i < 64u32 { state = state * 1664525u32 + 1013904223u32; i = i + 1u32; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_u32_normalize = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::NormalizeInt {
                signed: false,
                bits: 32,
                ..
            }
        )
    });
    assert!(has_u32_normalize);
}

#[test]
fn lower_runtime_generic_struct_const_field_access() {
    let src = "struct Cfg { mul: u64; add: u64; } fn main() { let cfg: Cfg = Cfg { mul: 1664525u64, add: 1013904223u64 }; let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 32u64 { state = state * cfg.mul + cfg.add; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_mul = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::Mul,
                lhs: super::RuntimeOperand::Imm(1_664_525),
                ..
            }
        ) || matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::Mul,
                rhs: super::RuntimeOperand::Imm(1_664_525),
                ..
            }
        ) || matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Mul,
                rhs: super::RuntimeOperand::Imm(1_664_525),
                ..
            }
        )
    });
    let has_add = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Add,
                rhs: super::RuntimeOperand::Imm(1_013_904_223),
                ..
            }
        )
    });
    assert!(has_mul);
    assert!(has_add);
}

#[test]
fn lower_runtime_generic_struct_dynamic_field_access() {
    let src = "struct Cfg { mul: u64; } fn main() { let cfg: Cfg = Cfg { mul: runtime_seed() }; let mut state: u64 = 1u64; state = state + cfg.mul; exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::LoadSeed { .. }))
    );
}

#[test]
fn lower_runtime_generic_struct_field_assignment() {
    let src = "struct Pair { x: u64; y: u64; } fn main() { let mut p: Pair = Pair { x: runtime_seed(), y: 1u64 }; p.x = p.x + p.y; exit(p.x); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_runtime_generic_struct_whole_assignment_from_literal() {
    let src = "struct Pair { x: u64; y: u64; } fn main() { let mut p: Pair = Pair { x: runtime_seed(), y: 1u64 }; p = Pair { x: p.y, y: p.x }; exit(p.x); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn lower_runtime_generic_struct_whole_assignment_from_ident() {
    let src = "struct Pair { x: u64; y: u64; } fn main() { let mut p: Pair = Pair { x: runtime_seed(), y: 1u64 }; let q: Pair = Pair { x: 9u64, y: 3u64 }; p = q; exit(p.x); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_generic_struct_field_assignment_rejects_immutable_binding() {
    let src = "struct Pair { x: u64; } fn main() { let p: Pair = Pair { x: runtime_seed() }; p.x = 1u64; exit(p.x); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected immutable field assignment error");
    assert!(err.message.contains("cannot assign to immutable"));
}

#[test]
fn runtime_generic_struct_whole_assignment_rejects_immutable_binding() {
    let src = "struct Pair { x: u64; y: u64; } fn main() { let p: Pair = Pair { x: runtime_seed(), y: 1u64 }; p = Pair { x: 1u64, y: 2u64 }; exit(p.x); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected immutable struct assignment error");
    assert!(err.message.contains("cannot assign to immutable"));
}

#[test]
fn lower_runtime_generic_array_const_index_access() {
    let src = "fn main() { let lut: [u64; 4] = [1u64, 3u64, 7u64, 15u64]; let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 32u64 { state = state + lut[2u64]; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_lut_use = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Add,
                rhs: super::RuntimeOperand::Imm(7) | super::RuntimeOperand::Slot(_),
                ..
            }
        )
    });
    let has_dynamic_index_chain = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::Eq,
                ..
            }
        )
    });
    assert!(has_lut_use);
    assert!(!has_dynamic_index_chain);
}

#[test]
fn lower_runtime_generic_dict_const_key_access() {
    let src = "fn main() { let lut: dict<string, u64> = {\"alpha\": 5u64, \"beta\": 9u64}; let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 32u64 { state = state + lut[\"beta\"]; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_dict_const = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Add,
                rhs: super::RuntimeOperand::Imm(9),
                ..
            }
        )
    });
    assert!(has_dict_const);
}

#[test]
fn lower_runtime_generic_const_container_len_method() {
    let src = "fn main() { let lut: [u64; 4] = [1u64, 2u64, 3u64, 4u64]; let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < lut.len() { state = state + 1u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_len_imm = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                rhs: super::RuntimeOperand::Imm(4),
                ..
            }
        )
    });
    assert!(has_len_imm);
}

#[test]
fn lower_runtime_generic_const_container_is_empty_method() {
    let src = "fn main() { let lut: [u64; 1] = [5u64]; let mut state: u64 = runtime_seed(); if lut.is_empty() { state = state + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_runtime_len_cmp = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Cmp {
                op: super::RuntimeCmpOp::Eq,
                rhs: super::RuntimeOperand::Imm(0),
                ..
            }
        )
    });
    assert!(!has_runtime_len_cmp);
}

#[test]
fn lower_runtime_generic_bool_return_call() {
    let src = "fn ready() -> bool { return true; } fn main() { let mut state: u64 = runtime_seed(); if ready() { state = state + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_call = program
        .instrs
        .iter()
        .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }));
    assert!(has_call);
}

#[test]
fn lower_runtime_generic_foreach_const_array() {
    let src = "fn main() { let arr: [u64; 3] = [2u64, 3u64, 5u64]; let mut state: u64 = runtime_seed(); foreach x in arr { state = state + x; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let const_moves: Vec<u64> = program
        .instrs
        .iter()
        .filter_map(|instr| match instr {
            super::RuntimeInstr::Mov {
                src: super::RuntimeOperand::Imm(v),
                ..
            } => Some(*v),
            _ => None,
        })
        .collect();
    assert!(const_moves.contains(&2));
    assert!(const_moves.contains(&3));
    assert!(const_moves.contains(&5));
}

#[test]
fn lower_runtime_generic_dynamic_array_index_emits_bounds_checks() {
    let src = "fn main() { let arr: [u64; 4] = [11u64, 13u64, 17u64, 19u64]; let mut state: u64 = runtime_seed(); let idx: u64 = state; state = state + arr[idx]; exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_oob_exit = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Exit {
                code: super::RuntimeOperand::Imm(255)
            }
        )
    });
    let has_checked_load = program
        .instrs
        .iter()
        .any(|instr| matches!(instr, super::RuntimeInstr::LoadIndex { .. }));
    let eq_checks = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                super::RuntimeInstr::JumpIfCmpFalse {
                    op: super::RuntimeCmpOp::Eq,
                    ..
                }
            )
        })
        .count();
    assert!(has_checked_load);
    assert!(!has_oob_exit);
    assert_eq!(eq_checks, 0);
}

#[test]
fn lower_runtime_generic_for_range_uses_unchecked_array_index() {
    let src = "fn main() { let mut arr: [u64; 8] = [1u64,2u64,3u64,4u64,5u64,6u64,7u64,8u64]; let mut state: u64 = runtime_seed(); for i in 0u64..8u64 { state = state + arr[i]; state = state + arr[i]; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::LoadIndexUnchecked { .. }))
    );
}

#[test]
fn lower_runtime_generic_masked_index_uses_unchecked_index_ops() {
    let src = "fn main() { let mut arr: [u64; 8] = [1u64,2u64,3u64,4u64,5u64,6u64,7u64,8u64]; let mut idx: u64 = runtime_seed(); idx = idx & 7u64; arr[idx] = idx; let v: u64 = arr[idx]; exit(v); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::LoadIndexUnchecked { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::StoreIndexUnchecked { .. }))
    );
}

#[test]
fn lower_runtime_generic_bool_clear_if_is_branchless() {
    let src = "fn main() { let mut flag: u8 = 1u8; let x: u64 = runtime_seed(); if x == 0u64 { flag = 0u8; } exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::BitAnd,
                ..
            }
        )
    }));
    assert!(!program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfZero { .. } | super::RuntimeInstr::JumpIfCmpFalse { .. }
        )
    }));
}

#[test]
fn lower_runtime_generic_shift_mask_index_uses_unchecked_index_ops() {
    let src = "fn main() { let mut arr: [u64; 8] = [0u64,0u64,0u64,0u64,0u64,0u64,0u64,0u64]; let h: u64 = runtime_seed(); let bit_idx: u64 = h & 63u64; let word_idx: u64 = bit_idx >> 3u64; arr[word_idx] = 1u64; let v: u64 = arr[word_idx]; exit(v); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::LoadIndexUnchecked { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::StoreIndexUnchecked { .. }))
    );
}

#[test]
fn lower_runtime_generic_small_while_body_is_unrolled() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let arr: [u64; 2] = [1u64, 2u64]; let mut i: u64 = 0u64; while i < 4u64 { state = state + arr[i & 1u64]; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let while_cond_checks = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                super::RuntimeInstr::JumpIfCmpFalse {
                    op: super::RuntimeCmpOp::LtUnsigned,
                    ..
                }
            )
        })
        .count();
    assert!(while_cond_checks >= 2);
}

#[test]
fn lower_runtime_generic_branchless_zero_clear_for_u64_slot() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut flag: u64 = 1u64; if (state & 1u64) == 0u64 { flag = 0u64; } exit(flag); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(
        program.instrs.iter().any(|instr| {
            matches!(
                instr,
                super::RuntimeInstr::BinOpInPlace {
                    op: super::RuntimeBinOp::BitAnd,
                    ..
                }
            )
        }),
        "instrs={:?}",
        program.instrs
    );
    assert!(
        !program.instrs.iter().any(|instr| {
            matches!(
                instr,
                super::RuntimeInstr::JumpIfZero { .. } | super::RuntimeInstr::JumpIfCmpFalse { .. }
            )
        }),
        "instrs={:?}",
        program.instrs
    );
}

#[test]
fn lower_index_assignment_interpreter_path() {
    let src = "fn main() { let mut arr: [u64; 3] = [1u64, 2u64, 3u64]; arr[1u64] = 9u64; print(arr[1u64].to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "9"))
    );
}

#[test]
fn lower_runtime_generic_mutable_array_index_assignment() {
    let src = "fn main() { let mut arr: [u64; 4] = [1u64, 2u64, 3u64, 4u64]; let mut state: u64 = runtime_seed(); let idx: u64 = state; arr[idx] = state; state = state + arr[idx]; exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_slot_write = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Mov {
                src: super::RuntimeOperand::Slot(_),
                ..
            }
        )
    });
    let has_oob_exit = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Exit {
                code: super::RuntimeOperand::Imm(255)
            }
        )
    });
    let has_store_index = program
        .instrs
        .iter()
        .any(|instr| matches!(instr, super::RuntimeInstr::StoreIndex { .. }));
    assert!(has_slot_write);
    assert!(has_oob_exit || has_store_index);
}

#[test]
fn lower_method_calls_interpreter_array_and_dict() {
    let src = "fn main() { let mut arr: [u64; 2] = [1u64, 2u64]; let mut d: dict<string, u64> = {\"a\": 5u64}; arr.push(3u64); arr.pop(); d.set(\"a\", 9u64); d.remove(\"a\"); print(arr.len().to_str()); print(\"\\n\"); print(d.len().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "2"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "0"))
    );
}

#[test]
fn lower_runtime_generic_mutable_dict_index_assignment() {
    let src = "fn main() { let mut d: dict<string, u64> = {\"a\": 1u64, \"b\": 2u64}; let mut s: u64 = runtime_seed(); d[\"a\"] = s; s = s + d[\"a\"]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_dict_slot_write = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::Mov {
                src: super::RuntimeOperand::Slot(_),
                ..
            }
        )
    });
    assert!(has_dict_slot_write);
}

#[test]
fn lower_runtime_generic_array_push_pop_methods() {
    let src = "fn main() { let mut arr: [u64; 4] = [1u64, 2u64, 3u64, 4u64]; let mut s: u64 = runtime_seed(); arr.pop(); arr.push(s); s = s + arr.len(); exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_len_sub = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Sub,
                rhs: super::RuntimeOperand::Imm(1),
                ..
            }
        )
    });
    let has_len_add = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Add,
                rhs: super::RuntimeOperand::Imm(1),
                ..
            }
        )
    });
    assert!(has_len_sub);
    assert!(has_len_add);
}

#[test]
fn lower_runtime_generic_unsigned_division_op() {
    let src = "fn main() { let mut state: u32 = runtime_seed(); let mut i: u32 = 0u32; while i < 64u32 { state = state / 4u32; i = i + 1u32; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_unsigned_div = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::DivUnsigned,
                ..
            }
        )
    });
    assert!(has_unsigned_div);
}

#[test]
fn lower_runtime_generic_signed_division_op() {
    let src = "fn main() { let mut state: i32 = runtime_seed(); let mut i: i32 = -8i32; while i < 8i32 { state = state / 2i32; i = i + 1i32; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_signed_div = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::DivSigned,
                ..
            }
        )
    });
    assert!(has_signed_div);
}

#[test]
fn lower_expression_logical_short_circuit() {
    let src = "fn main() { let a: bool = false && ((1i32 / 0i32) == 0i32); let b: bool = true || ((1i32 / 0i32) == 0i32); print(a.to_str()); print(\"\\n\"); print(b.to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "false"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "true"))
    );
}

#[test]
fn lower_runtime_generic_bitwise_mod_shift_ops() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 64u64 { state = ((state ^ i) & 255u64) | (state % 7u64); state = state << 1u64; state = state >> 1u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };

    let has_xor = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::BitXor,
                ..
            } | super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::BitXor,
                ..
            }
        )
    });
    let has_and = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::BitAnd,
                ..
            } | super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::BitAnd,
                ..
            }
        )
    });
    let has_or = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::BitOr,
                ..
            } | super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::BitOr,
                ..
            }
        )
    });
    let has_mod = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::ModUnsigned,
                ..
            } | super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::ModUnsigned,
                ..
            }
        )
    });
    let has_shl = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::Shl,
                ..
            } | super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Shl,
                ..
            }
        )
    });
    let has_shr = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::ShrUnsigned,
                ..
            } | super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::ShrUnsigned,
                ..
            }
        )
    });
    assert!(has_xor && has_and && has_or && has_mod && has_shl && has_shr);
}

#[test]
fn lower_runtime_generic_nested_in_place_chain() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 64u64 { if i < 32u64 { state = (state + i) + 2u64; } else { state = state + 1u64; } i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_add_with_slot = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Add,
                rhs: super::RuntimeOperand::Slot(_),
                ..
            }
        )
    });
    let has_add_two = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::BinOpInPlace {
                op: super::RuntimeBinOp::Add,
                rhs: super::RuntimeOperand::Imm(2),
                ..
            }
        )
    });
    assert!(has_add_with_slot);
    assert!(has_add_two);
}

#[test]
fn runtime_seed_lowers_with_runtime_print_int() {
    let src = "fn main() { let x: u64 = runtime_seed(); print(x); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::PrintInt { .. }))
    );
}

#[test]
fn runtime_seed_lowers_with_runtime_print_const_and_int() {
    let src = "fn main() { let x: u64 = runtime_seed(); print(\"x=\"); print(x); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::PrintConst { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::PrintInt { .. }))
    );
}

#[test]
fn runtime_generic_while_supports_break_and_continue() {
    let src = "fn main() { let mut i: u64 = runtime_seed(); while i < 20u64 { i = i + 1u64; if i == 5u64 { continue; } if i == 9u64 { break; } } exit(i); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Jump { .. }))
    );
}

#[test]
fn runtime_generic_loop_supports_break_and_continue() {
    let src = "fn main() { let mut i: u64 = runtime_seed(); loop { i = i + 1u64; if i == 3u64 { continue; } if i == 7u64 { break; } } exit(i); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Jump { .. }))
    );
}

#[test]
fn runtime_generic_parfor_body_lowers() {
    let src = "fn main() { let mut s: u64 = runtime_seed(); parfor i in 0u64..4u64 { s = s + i; if i == 2u64 { continue; } } exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::JumpIfCmpFalse { .. }))
    );
}

#[test]
fn runtime_generic_parfor_reduction_lowers() {
    let src = "fn main() { let mut total: u64 = 0u64; let s: u64 = runtime_seed(); parfor i in 0u64..4u64 reduce sum into total { i + s }; exit(total); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::JumpIfCmpFalse {
            op: super::RuntimeCmpOp::Eq,
            ..
        }
    )));
}

#[test]
fn runtime_generic_parfor_reduction_requires_mutable_target() {
    let src = "fn main() { let total: u64 = 0u64; let s: u64 = runtime_seed(); parfor i in 0u64..4u64 reduce sum into total { i + s }; exit(total); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected immutable reduction target error");
    assert!(err.message.contains("must be mutable"));
}

#[test]
fn runtime_generic_parfor_reduction_requires_integer_target() {
    let src = "fn main() { let mut total: f64 = 0.0f64; let s: u64 = runtime_seed(); parfor i in 0u64..4u64 reduce sum into total { 1.0f64 }; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected non-integer reduction target error");
    assert!(
        err.message
            .contains("parfor reductions require integer targets")
    );
}

#[test]
fn lower_runtime_heap_alloc_free_to_runtime_instrs() {
    let src = "fn main() { let p: u64 = heap_alloc(64u64); heap_free(p, 64u64); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Alloc { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
    );
    assert!(program.instrs.windows(2).any(|window| matches!(
        window,
        [
            super::RuntimeInstr::Free { .. },
            super::RuntimeInstr::Mov {
                src: super::RuntimeOperand::Imm(0),
                ..
            }
        ]
    )));
}

#[test]
fn lower_resource_safe_file_lifecycle_to_runtime_instrs() {
    let src = "fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let text: string = \"hello\"; let output: File = file_create(path); let written: u64 = file_write_all(output, text); file_close(output); let input: File = file_open_read(path); let content: string = file_read(input, 16u64); file_close(input); exit(written + content.len()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert_eq!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::FileOpen { .. }))
            .count(),
        2,
        "{:#?}",
        program.instrs
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileWrite { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileRead { .. }))
    );
    let read_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::FileRead { .. }))
        .expect("missing file read");
    let read_failure_exit = program
        .instrs
        .iter()
        .enumerate()
        .skip(read_index + 1)
        .find_map(|(index, instr)| {
            matches!(
                instr,
                super::RuntimeInstr::Exit {
                    code: super::RuntimeOperand::Imm(104)
                }
            )
            .then_some(index)
        })
        .expect("missing deterministic read failure exit");
    assert!(
        program.instrs[read_index + 1..read_failure_exit]
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::FileClose { .. }))
            .count()
            >= 2
    );
}

#[test]
fn lower_opaque_file_inherent_api_without_consuming_borrowed_inputs() {
    let src = "fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let text: string = \"hello\"; let output: File = File::create(path); let written: u64 = output.write_all(text); if path.len() + text.len() == 0u64 { exit(1u64); } output.close(); let input: File = File::open_read(path); let content: string = input.read(16u64); input.close(); exit(written + content.len()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("opaque File API must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert_eq!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::FileOpen { .. }))
            .count(),
        2
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileWrite { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileRead { .. }))
    );
}

#[test]
fn path_is_opaque_validated_and_accepted_by_file_operations() {
    let src = "fn main() { let raw: string = \"/tmp/aziky-file-runtime\"; let path: Path = Path::new(raw); let file: File = File::create(path); file.close(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("Path-backed File operation must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileOpen { .. }))
    );
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::Exit {
            code: super::RuntimeOperand::Imm(105)
        }
    )));

    let forged = "fn main() { let path: Path = \"/tmp/aziky-file-runtime\"; exit(0u64); }";
    let tokens = lex(forged).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("forged Path must fail");
    assert!(error.message.contains("opaque runtime resource"));
}

#[test]
fn path_join_consumes_base_and_borrows_validated_segment() {
    let src = "fn main() { let raw: string = \"/tmp\"; let base: Path = Path::new(raw); let segment: string = \"aziky\"; let joined: Path = base.join(segment); let file: File = File::create(joined); if segment.len() == 0u64 { exit(1u64); } file.close(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    lower_program(&program).expect("Path::join must lower natively");

    let reused = "fn main() { let raw: string = \"/tmp\"; let base: Path = Path::new(raw); let segment: string = \"aziky\"; let joined: Path = base.join(segment); let file: File = File::create(base); file.close(); let joined_file: File = File::create(joined); joined_file.close(); exit(0u64); }";
    let tokens = lex(reused).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("joined base reuse must fail");
    assert!(error.message.contains("moved or consumed"));
}

#[test]
fn runtime_owned_file_is_closed_automatically_and_consumed_once() {
    let cleanup_src = "fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = file_create(path); exit(0u64); }";
    let tokens = lex(cleanup_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    let open_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::FileOpen { .. }))
        .expect("missing file open");
    let close_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::FileClose { .. }))
        .expect("missing automatic close");
    let exit_index = program
        .instrs
        .iter()
        .rposition(|instr| matches!(instr, super::RuntimeInstr::Exit { .. }))
        .expect("missing exit");
    assert!(
        open_index < close_index && close_index < exit_index,
        "{:#?}",
        program.instrs
    );

    let move_src = "fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = file_create(path); let moved: File = file; file_close(moved); exit(0u64); }";
    let tokens = lex(move_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    lower_program(&program).expect("file ownership move must lower");

    let double_close_src = "fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = file_create(path); file_close(file); file_close(file); exit(0u64); }";
    let tokens = lex(double_close_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("double close must fail");
    assert!(error.message.contains("was moved or consumed"));
}

#[test]
fn runtime_owned_file_is_closed_before_function_return() {
    let src = "fn helper() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = file_create(path); return; } fn main() { helper(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    let open_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::FileOpen { .. }))
        .expect("missing file open");
    let return_index = program
        .instrs
        .iter()
        .enumerate()
        .skip(open_index + 1)
        .find_map(|(index, instr)| matches!(instr, super::RuntimeInstr::Return).then_some(index))
        .expect("missing helper return");
    assert!(
        program.instrs[open_index + 1..return_index]
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileClose { .. }))
    );
}

#[test]
fn opaque_file_moves_through_return_and_borrows_through_call_abi() {
    let src = "fn open(path: string) -> File { let file: File = File::open_read(path); return file; } fn read_some(file: &File) -> string { let content: string = file.read(8u64); return content; } fn consume(file: File) { file.close(); } fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = open(path); let content: string = read_some(&file); consume(file); exit(content.len()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("File call ABI must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileOpen { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileRead { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FileClose { .. }))
    );
}

#[test]
fn borrowed_file_cannot_be_closed_by_the_callee() {
    let src = "fn invalid(file: &File) { file.close(); } fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = File::open_read(path); invalid(&file); file.close(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("borrowed File close must fail");
    assert!(error.message.contains("requires a File owner"));
}

#[test]
fn opaque_file_rejects_integer_annotation_forgery_and_use_after_call_move() {
    let integer_src = "fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: u64 = File::open_read(path); exit(0u64); }";
    let tokens = lex(integer_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("integer-backed File must fail");
    assert!(error.message.contains("opaque File type"));

    let forged_src = "fn main() { let file: File = 3u64; exit(0u64); }";
    let tokens = lex(forged_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("forged File must fail");
    assert!(error.message.contains("opaque runtime resource"));

    let moved_src = "fn consume(file: File) { file.close(); } fn main() { let path: string = \"/tmp/aziky-file-runtime\"; let file: File = File::open_read(path); consume(file); file.close(); exit(0u64); }";
    let tokens = lex(moved_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("moved File reuse must fail");
    assert!(error.message.contains("was moved or consumed"));
}

#[test]
fn runtime_owned_heap_binding_is_cleaned_on_exit_without_manual_free() {
    let src = "fn main() { let p: u64 = heap_alloc(64u64); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    let alloc_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::Alloc { .. }))
        .expect("missing allocation");
    let free_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
        .expect("missing automatic cleanup");
    let exit_index = program
        .instrs
        .iter()
        .position(|instr| matches!(instr, super::RuntimeInstr::Exit { .. }))
        .expect("missing exit");
    assert!(alloc_index < free_index && free_index < exit_index);
}

#[test]
fn runtime_owned_heap_pointer_moves_and_consumes_once() {
    let move_src = "fn main() { let p: u64 = heap_alloc(64u64); let q: u64 = p; heap_free(q, 64u64); exit(0u64); }";
    let tokens = lex(move_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned pointer move must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        matches!(
            program.instrs.as_slice(),
            [
                ..,
                super::RuntimeInstr::Free { .. },
                super::RuntimeInstr::Mov {
                    src: super::RuntimeOperand::Imm(0),
                    ..
                },
                super::RuntimeInstr::Exit { .. }
            ]
        ),
        "the successful release path must not emit a second scope cleanup"
    );

    let moved_source_src = "fn main() { let p: u64 = heap_alloc(64u64); let q: u64 = p; heap_free(p, 64u64); exit(0u64); }";
    let tokens = lex(moved_source_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("moved owner use must fail");
    assert!(error.message.contains("was moved or consumed"));

    let double_consume_src = "fn main() { let p: u64 = heap_alloc(64u64); heap_free(p, 64u64); heap_free(p, 64u64); exit(0u64); }";
    let tokens = lex(double_consume_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("double release must fail");
    assert!(error.message.contains("was moved or consumed"));

    let nested_consume_src = "fn main() { let p: u64 = heap_alloc(64u64); if runtime_seed() == 0u64 { heap_free(p, 64u64); } exit(0u64); }";
    let tokens = lex(nested_consume_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("branch-local release of outer owner must fail");
    assert!(error.message.contains("current lexical scope"));

    let mutable_move_src =
        "fn main() { let p: u64 = heap_alloc(64u64); let mut q: u64 = p; exit(0u64); }";
    let tokens = lex(mutable_move_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("mutable moved owner must fail");
    assert!(error.message.contains("cannot be mutable or reassigned"));

    let raw_free_src = "fn main() { heap_free(42u64, 64u64); exit(0u64); }";
    let tokens = lex(raw_free_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("raw integer free must fail");
    assert!(error.message.contains("requires the named owner"));
}

#[test]
fn runtime_owned_heap_conditional_exit_keeps_implicit_fallthrough_cleanup() {
    let src =
        "fn main() { let p: u64 = heap_alloc(64u64); if runtime_seed() == 0u64 { exit(1u64); } }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(matches!(
        program.instrs.last(),
        Some(super::RuntimeInstr::Exit {
            code: super::RuntimeOperand::Imm(0)
        })
    ));
    assert!(program.instrs.windows(3).any(|window| matches!(
        window,
        [
            super::RuntimeInstr::Free { .. },
            super::RuntimeInstr::Mov {
                src: super::RuntimeOperand::Imm(0),
                ..
            },
            super::RuntimeInstr::Exit {
                code: super::RuntimeOperand::Imm(0)
            }
        ]
    )));
}

#[test]
fn runtime_owned_lists_move_without_copying_or_duplicate_cleanup() {
    let src = "fn main() { let mut values: list<u16> = [1u16, 2u16]; let mut moved: list<u16> = values; moved.push(3u16); let mut total: u16 = 0u16; foreach value in moved { total = total + value; } exit(total); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned list move must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(matches!(
        program.instrs.last(),
        Some(super::RuntimeInstr::Exit { .. })
    ));

    let moved_source_src = "fn main() { let seed: u64 = runtime_seed(); let mut values: list<u16> = [1u16]; let mut moved: list<u16> = values; values.push(2u16); exit(0u16); }";
    let tokens = lex(moved_source_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("moved list use must fail");
    assert!(error.message.contains("was moved or consumed"));

    let mismatched_type_src = "fn main() { let seed: u64 = runtime_seed(); let mut values: list<u16> = [1u16]; let mut moved: list<u32> = values; exit(0u16); }";
    let tokens = lex(mismatched_type_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("mismatched list move must fail");
    assert!(error.message.contains("does not match its owner"));
}

#[test]
fn runtime_owned_list_arguments_move_into_native_function_abi() {
    let src = "fn consume(values: list<u16>) -> u64 { return values.len(); } fn main() { let seed: u64 = runtime_seed(); let mut values: list<u16> = [1u16, 2u16, 3u16]; let count: u64 = consume(values); exit(count); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned list argument move must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }))
    );

    let reuse_src = "fn consume(values: list<u16>) -> u64 { return values.len(); } fn main() { let seed: u64 = runtime_seed(); let mut values: list<u16> = [1u16]; let count: u64 = consume(values); values.push(2u16); exit(count); }";
    let tokens = lex(reuse_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("moved call argument must be unusable");
    assert!(error.message.contains("was moved or consumed"));
}

#[test]
fn runtime_owned_list_returns_move_back_through_native_abi() {
    let src = "fn make() -> list<u16> { let mut values: list<u16> = [4u16, 5u16]; return values; } fn main() { let seed: u64 = runtime_seed(); let mut result: list<u16> = make(); result.push(6u16); let mut total: u16 = 0u16; foreach value in result { total = total + value; } exit(total); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned list return must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_struct_list_returns_move_back_through_native_abi() {
    let src = "struct Pair { left: u16; right: u16; } fn make() -> list<Pair> { let mut values: list<Pair> = [Pair { left: 4u16, right: 5u16 }]; return values; } fn main() { let seed: u64 = runtime_seed(); let result: list<Pair> = make(); let first: Pair = result.first().unwrap_or(Pair { left: 0u16, right: 0u16 }); exit(first.left + first.right); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct-list return must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_loop_break_uses_one_cleanup_path_per_owner() {
    let src = "fn main() { let seed: u64 = runtime_seed(); for index in 0u64..2u64 { let allocation: u64 = heap_alloc(8u64); break; } exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned break cleanup must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    // One Free belongs to allocation-failure cleanup. The loop-local owner then
    // has one cleanup instruction for the `break` path and one for a normal
    // completed iteration; there must be no post-loop duplicate.
    assert_eq!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
            .count(),
        3
    );
}

#[test]
fn runtime_scalar_structs_cross_native_call_and_return_abi() {
    let src = "struct Pair { left: u16; right: u16; } fn add(pair: Pair) -> Pair { let mut result: Pair = pair; result.left = result.left + 2u16; result.right = result.right + 3u16; return result; } fn main() { let seed: u64 = runtime_seed(); let mut input: Pair = Pair { left: 4u16, right: 5u16 }; let output: Pair = add(input); exit(output.left + output.right); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("native struct call and return must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_struct_literal_owns_and_cleans_nested_list_field() {
    let src = "struct Bag { count: u64; values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16, 2u16, 3u16] }; exit(bag.count & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct literal must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Alloc { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
    );
}

#[test]
fn runtime_owned_struct_moves_nested_list_owner_without_copying_descriptor() {
    let src = "struct Bag { count: u64; values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16, 2u16] }; let moved: Bag = bag; exit(moved.count & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct move must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));

    let reuse_src = "struct Bag { count: u64; values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16, 2u16] }; let moved: Bag = bag; exit(bag.count & 0u64); }";
    let tokens = lex(reuse_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("moved owned struct must be unusable");
    assert!(error.message.contains("was moved or consumed"));
}

#[test]
fn runtime_owned_struct_moves_through_native_call_and_return_abi() {
    let src = "struct Bag { count: u64; values: list<u16>; } fn pass(bag: Bag) -> Bag { return bag; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16, 2u16] }; let result: Bag = pass(bag); exit(result.count & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct call and return must lower");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }))
    );

    let reuse_src = "struct Bag { count: u64; values: list<u16>; } fn consume(bag: Bag) -> u64 { return bag.count; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { count: seed, values: [1u16] }; let count: u64 = consume(bag); exit(bag.count & count); }";
    let tokens = lex(reuse_src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error =
        lower_program(&program).expect_err("moved owned struct call argument must be unusable");
    assert!(error.message.contains("was moved or consumed"));
}

#[test]
fn runtime_owned_struct_list_field_len_and_empty_lower_natively() {
    let src = "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [1u16, 2u16] }; let count: u64 = bag.values.len(); let second: u16 = bag.values[1u64]; let empty: bool = bag.values.is_empty(); if empty { exit(1u64); } if second == 0u16 { exit(1u64); } exit(count + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct list field queries must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_struct_list_field_mutation_uses_native_owner_descriptor() {
    let src = "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [1u16] }; bag.values.reserve(8u64); bag.values.push(2u16); bag.values[0u64] = 4u16; bag.values.pop(); bag.values.push(3u16); bag.values.shrink_to(2u64); bag.values.shrink_to_fit(); let value: u16 = bag.values[1u64]; if value == 0u16 { exit(1u64); } exit(bag.values.len() + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct list field mutation must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));

    let immutable = "struct Bag { values: list<u16>; } fn main() { let bag: Bag = Bag { values: [1u16] }; bag.values.push(2u16); exit(0u64); }";
    let tokens = lex(immutable).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("immutable list field mutation must fail");
    assert!(error.message.contains("cannot mutate immutable"));
}

#[test]
fn runtime_owned_struct_struct_list_field_mutation_lowers_natively() {
    let src = "struct Pair { left: u16; right: u16; } struct Bag { values: list<Pair>; } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [Pair { left: 1u16, right: 2u16 }] }; bag.values.reserve(8u64); bag.values.push(Pair { left: 3u16, right: 4u16 }); bag.values.pop(); bag.values.push(Pair { left: 5u16, right: 6u16 }); bag.values.shrink_to(2u64); bag.values.shrink_to_fit(); exit(bag.values.len() + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct-list field mutation must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_struct_struct_list_field_index_assignment_lowers_natively() {
    let src = "struct Pair { left: u16; right: u16; } struct Bag { values: list<Pair>; } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { values: [Pair { left: 1u16, right: 2u16 }] }; bag.values[0u64] = Pair { left: 4u16, right: 5u16 }; exit(bag.values.len() + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("aggregate index assignment must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_struct_list_field_foreach_lowers_without_temporary_owner() {
    let src = "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [2u16, 3u16, 4u16] }; let mut total: u16 = 0u16; foreach value in bag.values { total = total + value; } if total == 0u16 { exit(1u64); } exit(bag.values.len() + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("owned struct list field foreach must lower");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_struct_list_field_contains_lowers_natively() {
    let src = "struct Bag { values: list<u16>; } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [2u16, 3u16] }; let found: bool = bag.values.contains(3u16); if !found { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("contains must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_u64_list_lowers_growth_access_and_cleanup() {
    let src = "fn main() { let mut values: list<u64> = []; values.push(1u64); values.push(2u64); values.push(3u64); values.push(4u64); values.push(5u64); values[1u64] = 9u64; let n: u64 = values.len(); let x: u64 = values[1u64]; values.pop(); exit(x + n); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Alloc { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::HeapCopy { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::HeapStoreInt { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::HeapLoadInt { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
    );
}

#[test]
fn runtime_owned_u64_list_lowers_foreach_contains_and_clear() {
    let src = "fn main() { let mut values: list<u64> = []; values.push(2u64); values.push(4u64); values.push(6u64); values.reserve(10u64); values.shrink_to(5u64); let mut total: u64 = 0u64; foreach value in values { total = total + value; } if values.contains(4u64) { total = total + 10u64; } values.clear(); values.shrink_to_fit(); exit(total + values.len()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::HeapLoadInt { .. }))
            .count()
            >= 2,
        "foreach and contains must both read list elements from owned storage"
    );
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::JumpIfCmpFalse {
            op: super::RuntimeCmpOp::LtUnsigned,
            ..
        }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::JumpIfCmpFalse {
            op: super::RuntimeCmpOp::Eq,
            ..
        }
    )));
    assert!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::Alloc { .. }))
            .count()
            >= 3,
        "growth, reserve, and non-empty shrink paths must allocate owned storage"
    );
}

#[test]
fn runtime_owned_u64_list_lowers_checked_option_operations() {
    let src = "fn main() { let mut values: list<u64> = [10u64, 20u64, 30u64]; let got = values.get(1u64); let missing: Option<u64> = values.get(9u64); let head: Option<u64> = values.first(); let tail: Option<u64> = values.last(); let peeked: Option<u64> = values.peek(); let popped: Option<u64> = values.pop(); let second: Option<u64> = values.pop(); let first: Option<u64> = values.pop(); let absent: Option<u64> = values.pop(); let mut manual: Option<u64> = Option::None; manual = Option::Some(7u64); let mut total: u64 = got.unwrap_or(0u64) + missing.unwrap_or(4u64) + head.unwrap_or(0u64) + tail.unwrap_or(0u64) + peeked.unwrap_or(0u64) + popped.unwrap_or(0u64) + second.unwrap_or(0u64) + first.unwrap_or(0u64) + absent.unwrap_or(5u64) + manual.unwrap_or(0u64); if got.is_some() { total = total + 1u64; } if missing.is_none() { total = total + 1u64; } exit(total + values.get(0u64).unwrap_or(9u64) + values.len()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::HeapLoadInt { .. }))
            .count()
            >= 8,
        "checked access and successful pops must load live list elements"
    );
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::Cmp {
            op: super::RuntimeCmpOp::LtUnsigned,
            ..
        }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::Cmp {
            op: super::RuntimeCmpOp::GtUnsigned,
            ..
        }
    )));
}

#[test]
fn runtime_owned_u64_list_rejects_value_pop_from_immutable_binding() {
    let src = "fn main() { let seed: u64 = runtime_seed(); let values: list<u64> = [1u64]; let popped: Option<u64> = values.pop(); exit(popped.unwrap_or(0u64) + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("immutable value pop must fail");
    assert!(
        error.message.contains("cannot mutate immutable 'values'"),
        "unexpected diagnostic: {}",
        error.message
    );
}

#[test]
fn runtime_owned_integer_list_preserves_signed_narrow_element_types() {
    let src = "fn main() { let mut values: list<i8> = [-5i8, 7i8, -2i8]; values.push(4i8); values.push(-8i8); values.reserve(6u64); values.shrink_to_fit(); let head = values.first(); let tail: Option<i8> = values.last(); let peeked = values.peek(); let popped: Option<i8> = values.pop(); let missing = values.get(99u8); let mut total: i8 = 0i8; foreach value in values { total = total + value; } total = total + popped.unwrap_or(0i8) + missing.unwrap_or(6i8) + head.unwrap_or(0i8) + tail.unwrap_or(0i8) + peeked.unwrap_or(0i8) + 32i8; if values.contains(-2i8) { total = total + 1i8; } exit(total); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering");
    };

    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::NormalizeInt {
            signed: true,
            bits: 8,
            ..
        }
    )));
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::HeapLoadInt { bytes: 1, .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::HeapStoreInt { bytes: 1, .. }))
    );
}

#[test]
fn runtime_owned_integer_lists_use_packed_element_widths() {
    let cases = [
        ("i8", "-1i8", 1u8),
        ("u16", "2u16", 2u8),
        ("i32", "-3i32", 4u8),
        ("u64", "4u64", 8u8),
    ];

    for (ty, value, expected_bytes) in cases {
        let src = format!(
            "fn main() {{ let mut values: list<{ty}> = [{value}]; values.push({value}); let item: {ty} = values[0u8]; exit(item); }}"
        );
        let tokens = lex(&src).expect("lex failed");
        let parsed = parse(&tokens).expect("parse failed");
        let lowered = lower_program(&parsed).expect("lower failed");
        let super::LoweredStmt::RuntimeGeneric { program } =
            lowered.first().expect("missing lowered runtime stmt")
        else {
            panic!("expected RuntimeGeneric lowering for list<{ty}>");
        };

        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapLoadInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapStoreInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::BinOp {
                op: super::RuntimeBinOp::Mul,
                rhs: super::RuntimeOperand::Imm(bytes),
                ..
            } if *bytes == u64::from(expected_bytes)
        )));
    }
}

#[test]
fn runtime_owned_float_lists_lower_packed_storage_and_checked_operations() {
    let cases = [
        (
            "f32",
            "fn main() { let mut values: list<f32> = [1.5f32, -0.0f32, 2.25f32, 3.5f32]; values.push(4.75f32); values.reserve(5u64); values.shrink_to_fit(); values[0u8] = 6.5f32; let head: Option<f32> = values.first(); let popped = values.pop(); let missing: Option<f32> = values.get(99u8); let mut total: f32 = 0.0f32; foreach value in values { total = total + value; } let combined: f32 = total + popped.unwrap_or(0.0f32) + missing.unwrap_or(8.0f32); if values.contains(head.unwrap_or(0.0f32)) && values.contains(0.0f32) && !values.contains(combined) && head.is_some() && missing.is_none() { exit(1u8); } exit(0u8); }",
            4u8,
        ),
        (
            "f64",
            "fn main() { let mut values: list<f64> = [-0.0f64, 1.25f64, 2.5f64, 3.75f64]; values.push(5.0f64); values.shrink_to_fit(); let popped: Option<f64> = values.pop(); let tail = values.last(); let mut total: f64 = 0.0f64; foreach value in values { total = total + value; } let nan: f64 = 0.0f64 / 0.0f64; values.push(nan); if values.contains(0.0f64) && values.contains(tail.unwrap_or(0.0f64)) && !values.contains(total + popped.unwrap_or(0.0f64)) && !values.contains(nan) { exit(1u8); } exit(0u8); }",
            8u8,
        ),
    ];

    for (ty, src, expected_bytes) in cases {
        let tokens = lex(src).expect("lex failed");
        let parsed = parse(&tokens).expect("parse failed");
        let lowered = lower_program(&parsed).expect("lower failed");
        let super::LoweredStmt::RuntimeGeneric { program } =
            lowered.first().expect("missing lowered runtime stmt")
        else {
            panic!("expected RuntimeGeneric lowering for list<{ty}>");
        };
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapLoadInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapStoreInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(
            program
                .instrs
                .iter()
                .any(|instr| matches!(instr, super::RuntimeInstr::FloatBinOp { .. }))
        );
    }
}

#[test]
fn runtime_owned_nested_struct_lists_lower_flattened_aos_and_checked_options() {
    let src = "struct Point { x: u16; score: f32; weight: f64; } struct Record { id: u8; point: Point; } fn main() { let mut values: list<Record> = [Record { id: 1u8, point: Point { x: 10u16, score: 1.5f32, weight: 1.0f64 } }, Record { id: 2u8, point: Point { x: 20u16, score: 2.5f32, weight: 2.0f64 } }]; values.push(Record { id: 3u8, point: Point { x: 30u16, score: 3.5f32, weight: 3.0f64 } }); values.reserve(4u64); values[1u8] = Record { id: 4u8, point: Point { x: 40u16, score: 4.5f32, weight: 4.0f64 } }; let picked: Record = values[1u8]; let got: Option<Record> = values.get(1u8); let missing: Option<Record> = values.get(99u8); let chosen = got.unwrap_or(Record { id: 0u8, point: Point { x: 1u16, score: 0.0f32, weight: 1.0f64 } }); let fallback = missing.unwrap_or(Record { id: 5u8, point: Point { x: 6u16, score: 0.0f32, weight: 7.0f64 } }); let mut total: f64 = 0.0f64; foreach row in values { total = total + row.point.weight; } if total == 8.0f64 { if picked.point.x == 40u16 { if chosen.point.x == 40u16 { if fallback.point.x == 6u16 { exit(picked.id + fallback.id); } } } } exit(0u8); }";
    let tokens = lex(src).expect("lex failed");
    let parsed = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&parsed).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering for nested list<Record>");
    };

    for expected_bytes in [1u8, 2u8, 4u8, 8u8] {
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapLoadInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapStoreInt { bytes, .. } if *bytes == expected_bytes
        )));
    }
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::BinOp {
            op: super::RuntimeBinOp::Mul,
            rhs: super::RuntimeOperand::Imm(16),
            ..
        }
    )));
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FloatBinOp { .. }))
    );
}

#[test]
fn runtime_owned_struct_lists_lower_aligned_aos_storage_and_copy_out() {
    let src = "struct Pair { small: u8; wide: u64; mid: u16; } fn main() { let mut values: list<Pair> = [Pair { small: 1u8, wide: 10u64, mid: 2u16 }, Pair { small: 3u8, wide: 20u64, mid: 4u16 }, Pair { small: 5u8, wide: 30u64, mid: 6u16 }, Pair { small: 7u8, wide: 40u64, mid: 8u16 }]; values.push(Pair { small: 9u8, wide: 50u64, mid: 10u16 }); values.reserve(5u64); values.shrink_to_fit(); values[1u8] = Pair { small: 11u8, wide: 40u64, mid: 12u16 }; let picked: Pair = values[1u8]; let mut total: u64 = 0u64; foreach pair in values { total = total + pair.wide; } if picked.small == 11u8 { if picked.wide == 40u64 { if picked.mid == 12u16 { if values.len() == 5u64 { exit(total); } } } } exit(0u8); }";
    let tokens = lex(src).expect("lex failed");
    let parsed = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&parsed).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering for list<Pair>");
    };

    for expected_bytes in [1u8, 2u8, 8u8] {
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapLoadInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapStoreInt { bytes, .. } if *bytes == expected_bytes
        )));
    }
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::HeapCopy { .. }))
    );
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::BinOp {
            op: super::RuntimeBinOp::Mul,
            rhs: super::RuntimeOperand::Imm(24),
            ..
        }
    )));
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Free { .. }))
    );
}

#[test]
fn runtime_owned_struct_lists_lower_checked_aggregate_options() {
    let src = "struct Pair { small: u16; wide: u64; } fn main() { let mut values: list<Pair> = [Pair { small: 1u16, wide: 10u64 }, Pair { small: 2u16, wide: 20u64 }, Pair { small: 3u16, wide: 30u64 }]; let got = values.get(1u8); let missing: Option<Pair> = values.get(99u8); let head: Option<Pair> = values.first(); let tail = values.last(); let peeked: Option<Pair> = values.peek(); let popped: Option<Pair> = values.pop(); let absent = values.get(99u8); let mut manual: Option<Pair> = Option::None; manual = Option::Some(Pair { small: 7u16, wide: 7u64 }); let got_value: Pair = got.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let missing_value = missing.unwrap_or(Pair { small: 0u16, wide: 4u64 }); let head_value = head.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let tail_value = tail.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let peek_value = peeked.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let popped_value = popped.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let absent_value = absent.unwrap_or(Pair { small: 0u16, wide: 5u64 }); let manual_value = manual.unwrap_or(Pair { small: 0u16, wide: 1u64 }); let direct_value: Pair = values.get(0u8).unwrap_or(Pair { small: 0u16, wide: 1u64 }); let mut total: u64 = got_value.wide + missing_value.wide + head_value.wide + tail_value.wide + peek_value.wide + popped_value.wide + absent_value.wide + manual_value.wide + direct_value.wide; if got.is_some() { total = total + 1u64; } if missing.is_none() { total = total + 1u64; } exit(total + values.len()); }";
    let tokens = lex(src).expect("lex failed");
    let parsed = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&parsed).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering for aggregate list options");
    };

    assert!(
        program
            .instrs
            .iter()
            .filter(|instr| matches!(instr, super::RuntimeInstr::HeapLoadInt { .. }))
            .count()
            >= 16,
        "checked aggregate access must load each field only on present paths"
    );
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::Cmp {
            op: super::RuntimeCmpOp::LtUnsigned,
            ..
        }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        super::RuntimeInstr::Cmp {
            op: super::RuntimeCmpOp::GtUnsigned,
            ..
        }
    )));
}

#[test]
fn runtime_owned_float_struct_lists_lower_packed_fields_and_options() {
    let src = "struct Sample { id: u16; score: f32; weight: f64; } fn main() { let mut values: list<Sample> = [Sample { id: 1u16, score: 1.5f32, weight: 1.0f64 }, Sample { id: 2u16, score: 2.5f32, weight: 2.0f64 }, Sample { id: 3u16, score: 3.5f32, weight: 3.0f64 }, Sample { id: 4u16, score: 4.5f32, weight: 4.0f64 }]; values.push(Sample { id: 5u16, score: 5.5f32, weight: 5.0f64 }); values.reserve(4u64); values.shrink_to_fit(); values[1u8] = Sample { id: 20u16, score: 20.5f32, weight: 20.0f64 }; let picked: Sample = values[1u8]; let got: Option<Sample> = values.get(1u8); let missing: Option<Sample> = values.get(99u8); let popped: Option<Sample> = values.pop(); let got_value = got.unwrap_or(Sample { id: 0u16, score: 0.0f32, weight: 1.0f64 }); let missing_value = missing.unwrap_or(Sample { id: 0u16, score: 0.0f32, weight: 7.0f64 }); let popped_value = popped.unwrap_or(Sample { id: 0u16, score: 0.0f32, weight: 1.0f64 }); let mut total: f64 = 0.0f64; foreach value in values { total = total + value.weight; } if total == 28.0f64 { exit(picked.id + got_value.id + missing_value.id + popped_value.id); } exit(0u8); }";
    let tokens = lex(src).expect("lex failed");
    let parsed = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&parsed).expect("lower failed");
    let super::LoweredStmt::RuntimeGeneric { program } =
        lowered.first().expect("missing lowered runtime stmt")
    else {
        panic!("expected RuntimeGeneric lowering for list<Sample>");
    };

    for expected_bytes in [2u8, 4u8, 8u8] {
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapLoadInt { bytes, .. } if *bytes == expected_bytes
        )));
        assert!(program.instrs.iter().any(|instr| matches!(
            instr,
            super::RuntimeInstr::HeapStoreInt { bytes, .. } if *bytes == expected_bytes
        )));
    }
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::FloatBinOp { .. }))
    );
}

#[test]
fn runtime_owned_u64_list_rejects_immutable_mutation() {
    let src = "fn main() { let values: list<u64> = [1u64]; values.push(2u64); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("immutable list push must fail");
    assert!(error.message.contains("cannot mutate immutable 'values'"));
}

#[test]
fn heap_free_rejected_in_expression_context() {
    let src = "fn main() { let x: u64 = heap_free(0u64, 8u64); exit(x); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected heap_free expression rejection");
    assert!(err.message.contains("heap_free() does not return a value"));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_alloc_kernel() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let alloc_bytes: u64 = 65536u64; let mut i: u64 = 0u64; while i < 2048u64 { state = state * 1664525u64 + 1013904223u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededLcgAllocLoop {
            iterations,
            mul,
            add,
            alloc_bytes,
            exit_with_state
        }) if *iterations == 2_048 && *mul == 1_664_525 && *add == 1_013_904_223 && *alloc_bytes == 65_536 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_affine_index_kernel_mul_shift() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 1000u64 { state = state * 8u64; state = state + i; state = state * 4u64; state = state - 3u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let Some(super::LoweredStmt::RuntimeSeededAffineIndexLoop {
        iterations,
        index_init,
        state_mul,
        index_mul,
        add,
        exit_with_state,
        ..
    }) = lowered.first()
    else {
        panic!("expected RuntimeSeededAffineIndexLoop lowering");
    };
    assert_eq!(*iterations, 1_000);
    assert_eq!(*index_init, 0);
    assert_eq!(*state_mul, 32);
    assert_eq!(*index_mul, 4);
    assert_eq!(*add, u32::MAX - 2);
    assert!(*exit_with_state);
}

#[cfg(any())]
#[test]
fn lower_zero_trip_seeded_affine_preserves_runtime_seed() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 10u64; while i < 5u64 { state = state * 8u64; state = state + i; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededAffineIndexLoop {
            iterations: 0,
            index_init: 10,
            ..
        })
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_affine_index_kernel_nested_chain() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 1000u64 { state = state + i; state = state + 4u64; state = state * 4u64; i = i + 1u64; } exit(state); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededAffineIndexLoop {
            iterations,
            index_init,
            state_mul,
            index_mul,
            add,
            exit_with_state,
            ..
        }) if *iterations == 1_000 && *index_init == 0 && *state_mul == 4 && *index_mul == 4 && *add == 16 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_masked_lcg_kernel() {
    let src = "fn main() { let mut state: u64 = 123456789u64; let mut i: u64 = 0u64; while i < 1000u64 { state = (state * 1664525u64 + 1013904223u64) & 4294967295u64; i = i + 1u64; } exit(state & 127u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeLcgLoop {
            iterations,
            state_init,
            mul,
            add,
            exit_with_state,
            exit_mask,
        }) if *iterations == 1_000
            && *state_init == 123_456_789
            && *mul == 1_664_525
            && *add == 1_013_904_223
            && *exit_with_state
            && *exit_mask == Some(127)
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_masked_branch_kernel_to_lcg() {
    let src = "fn main() { let mut state: u64 = 123456789u64; let mut i: u64 = 0u64; while i < 1000u64 { if state < 9223372036854775808u64 { state = (state * 1664525u64 + 1013904223u64) & 4294967295u64; } else { state = (state * 22695477u64 + 1u64) & 4294967295u64; } i = i + 1u64; } exit(state & 127u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeLcgLoop {
            iterations,
            state_init,
            mul,
            add,
            exit_with_state,
            exit_mask,
        }) if *iterations == 1_000
            && *state_init == 123_456_789
            && *mul == 1_664_525
            && *add == 1_013_904_223
            && *exit_with_state
            && *exit_mask == Some(127)
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_masked_affine_index_kernel() {
    let src = "fn main() { let mut state: u64 = 123456789u64; let mut i: u64 = 0u64; while i < 1000u64 { state = ((state << 3u8) + i) & 288230376151711743u64; state = ((state << 2u8) - 3u64) & 288230376151711743u64; i = i + 1u64; } exit(state & 127u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeAffineIndexLoop {
            iterations,
            state_init,
            index_init,
            state_mul,
            index_mul,
            add,
            state_mask,
            exit_with_state,
            exit_mask,
        }) if *iterations == 1_000
            && *state_init == 123_456_789
            && *index_init == 0
            && *state_mul == 32
            && *index_mul == 4
            && *add == u32::MAX - 2
            && *state_mask == 288_230_376_151_711_743
            && *exit_with_state
            && *exit_mask == Some(127)
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_ring_write_kernel() {
    let src = "fn main() { let mut state: u64 = 123456789u64; let mut buf: [u64; 64] = [0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64]; let mask: u64 = 63u64; let mut i: u64 = 0u64; while i < 1000u64 { state = (state * 1664525u64 + 1013904223u64) & 4294967295u64; let idx: u64 = i & mask; buf[idx] = (state << 32u8) | state; i = i + 1u64; } exit((((state << 32u8) | state) ^ buf[0u8]) & 127u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeRingWriteLoop {
            iterations,
            state_init,
            index_init,
            mul,
            add,
            state_mask,
            ring_mask,
            value_shift,
            exit_mask,
        }) if *iterations == 1_000
            && *state_init == 123_456_789
            && *index_init == 0
            && *mul == 1_664_525
            && *add == 1_013_904_223
            && *state_mask == 4_294_967_295
            && *ring_mask == 63
            && *value_shift == 32
            && *exit_mask == 127
    ));

    let zero_trip_src = src.replace("while i < 1000u64", "while i < 0u64");
    let tokens = lex(&zero_trip_src).expect("zero-trip lex failed");
    let program = parse(&tokens).expect("zero-trip parse failed");
    let lowered = lower_program(&program).expect("zero-trip lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeRingWriteLoop { iterations: 0, .. })
    ));
}

#[cfg(any())]
#[test]
fn lower_runtime_seeded_dual_state_branch_kernel() {
    let src = "fn main() { let mut a: u64 = runtime_seed(); let mut b: u64 = runtime_seed(); let mut i: u64 = 0u64; while i < 1000u64 { if a < b { a = a + i; a = a + 1u64; b = b * 4u64; } else { a = a + 3u64; b = b + a; b = b + 2u64; } i = i + 1u64; } exit(a + b); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededDualStateBranchLoop {
            iterations,
            index_init,
            adaptive,
            branchless,
            exit_with_sum
        }) if *iterations == 1_000 && *index_init == 0 && *adaptive && !*branchless && *exit_with_sum
    ));
}

#[test]
fn lower_struct_embed_layout_flattening() {
    let src = "struct Sensor { id: u64; value: u64; } struct Module { embed Sensor; unit: u64; } fn main() { let m: Module = Module { id: 5u64, value: 9u64, unit: 11u64 }; print(m.id.to_str()); print(\"\\n\"); print(m.value.to_str()); print(\"\\n\"); print(m.unit.to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "5"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "9"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "11"))
    );
}

#[test]
fn lower_trait_impl_monomorphized_method_dispatch() {
    let src = "trait Calib { fn calibrate(self_ref: &Module) -> u64; } struct Module { value: u64; unit: u64; } impl Calib for Module { fn calibrate(self_ref: &Module) -> u64 { return self_ref.value + self_ref.unit; } } fn main() { let m: Module = Module { value: 2u64, unit: 3u64 }; print(m.calibrate().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "5"))
    );
}

#[test]
fn lower_trait_impl_signature_mismatch_rejected() {
    let src = "trait Calib { fn calibrate(self_ref: &Module) -> u64; } struct Module { value: u64; } impl Calib for Module { fn calibrate(self_ref: &Module) -> i64 { return 0i64; } } fn main() { exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected trait signature mismatch");
    assert!(err.message.contains("return type mismatch"));
}

#[test]
fn lower_inherent_constructor_query_and_mutation() {
    let src = "struct Counter { value: i32; } impl Counter { fn new(value: i32) -> Self { return Self { value: value }; } fn current(self: &Self) -> i32 { return self.value; } fn add(self: &mut Self, amount: i32) { self.value = self.value + amount; } } fn main() { let mut counter: Counter = Counter::new(4i32); counter.add(3i32); print(counter.current().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(value) if value == "7"))
    );
}

#[test]
fn lower_inherent_query_on_temporary_receiver() {
    let src = "struct Counter { value: i32; } impl Counter { fn new(value: i32) -> Self { return Self { value: value }; } fn current(self: &Self) -> i32 { return self.value; } } fn main() { print(Counter::new(7i32).current().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("temporary receiver should lower");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(value) if value == "7"))
    );
}

#[test]
fn lower_inherent_void_associated_call_statement() {
    let src = "struct Reporter { marker: u8; } impl Reporter { fn emit(message: string) { print(message); } } fn main() { Reporter::emit(\"ready\"); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(value) if value == "ready"))
    );
}

#[test]
fn lower_inherent_duplicate_method_rejected_across_blocks() {
    let src = "struct Point { x: i32; } impl Point { fn get(self: &Self) -> i32 { return self.x; } } impl Point { fn get(self: &Self) -> i32 { return self.x; } } fn main() { exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected duplicate method error");
    assert!(err.message.contains("method 'get' is already defined"));
}

#[test]
fn lower_inherent_impl_rejects_unknown_target() {
    let src = "impl Missing { fn create() -> u64 { return 0u64; } } fn main() { exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected unknown impl target error");
    assert!(err.message.contains("must be a known struct"));
}

#[test]
fn associated_call_diagnostics_preserve_source_spelling() {
    let src =
        "struct Point { x: i32; } fn main() { let point: Point = Point::missing(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected unknown associated function error");
    assert!(err.message.contains("unknown function 'Point::missing'"));
    assert!(!err.message.contains("Point__missing"));

    let src = "struct Point { x: i32; } impl Point { fn new(x: i32) -> Self { return Self { x: x }; } } fn main() { let point: Point = Point::new(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected associated arity error");
    assert!(
        err.message
            .contains("function 'Point::new' expects 1 argument, got 0")
    );
    assert!(!err.message.contains("Point__new"));
}

#[test]
fn lower_inherent_mut_method_rejects_immutable_receiver() {
    let src = "struct Counter { value: i32; } impl Counter { fn add(self: &mut Self, amount: i32) { self.value = self.value + amount; } } fn main() { let counter: Counter = Counter { value: 4i32 }; counter.add(3i32); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected immutable receiver error");
    assert!(
        err.message
            .contains("cannot call mut method 'add' on immutable 'counter'")
    );
}

#[test]
fn lower_inherent_shared_receiver_cannot_assign_fields() {
    let src = "struct Counter { value: i32; } impl Counter { fn invalid(self: &Self) { self.value = 5i32; } } fn main() { let counter: Counter = Counter { value: 4i32 }; counter.invalid(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("expected shared receiver mutation error");
    assert!(
        err.message
            .contains("cannot assign through shared reference 'self'")
    );
}

#[test]
fn lower_runtime_generic_associated_function_call() {
    let src = "struct Mixer { marker: u64; } impl Mixer { fn next(value: u64) -> u64 { return value + 1u64; } } fn main() { let seed: u64 = runtime_seed(); let mixed: u64 = Mixer::next(seed); exit(mixed); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let Some(super::LoweredStmt::RuntimeGeneric { program }) = lowered.first() else {
        panic!("expected runtime generic lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::Call { .. }))
    );
}

#[test]
fn runtime_inherent_shared_and_mut_receivers_lower_to_direct_calls() {
    let src = "struct Counter { value: i32; } impl Counter { fn current(self: &Self) -> i32 { return self.value; } fn add(self: &mut Self, amount: i32) { self.value = self.value + amount; } } fn main() { let seed: u64 = runtime_seed(); let mut counter: Counter = Counter { value: 4i32 }; counter.add(3i32); let value: i32 = counter.current(); if value != 7i32 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("receiver methods must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_mut_receiver_rejects_immutable_binding() {
    let src = "struct Counter { value: i32; } impl Counter { fn add(self: &mut Self, amount: i32) { self.value = self.value + amount; } } fn main() { let seed: u64 = runtime_seed(); let counter: Counter = Counter { value: 4i32 }; counter.add(3i32); exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("immutable receiver must be rejected");
    assert!(
        err.message
            .contains("cannot call mut method 'add' on immutable 'counter'")
    );
}

#[test]
fn runtime_resource_owning_struct_receivers_use_linear_borrow_abi() {
    let src = "struct Bag { count: u64; values: list<u16>; } impl Bag { fn add(self: &mut Self, value: u16) { self.values.push(value); self.count = self.count + 1u64; } fn len(self: &Self) -> u64 { return self.values.len(); } } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { count: 1u64, values: [3u16] }; bag.add(5u16); if bag.count != 2u64 { exit(1u64); } if bag.len() != 2u64 { exit(2u64); } if bag.values[1u64] != 5u16 { exit(3u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("resource-owning receiver borrow ABI must lower");
    let Some(super::LoweredStmt::RuntimeGeneric { program }) = lowered.first() else {
        panic!("resource-owning receiver borrow ABI must select runtime lowering");
    };
    for instr in &program.instrs {
        let target = match instr {
            super::RuntimeInstr::Jump { target }
            | super::RuntimeInstr::JumpIfZero { target, .. }
            | super::RuntimeInstr::JumpIfCmpFalse { target, .. }
            | super::RuntimeInstr::Call { target } => Some(*target),
            _ => None,
        };
        assert!(target.is_none_or(|target| target < program.instrs.len()));
    }
}

#[test]
fn runtime_mut_resource_receiver_rejects_immutable_owner() {
    let src = "struct Bag { values: list<u16>; } impl Bag { fn add(self: &mut Self, value: u16) { self.values.push(value); } } fn main() { let seed: u64 = runtime_seed(); let bag: Bag = Bag { values: [3u16] }; bag.add(5u16); exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("immutable resource owner must be rejected");
    assert!(
        err.message
            .contains("cannot call mut method 'add' on immutable 'bag'")
    );
}

#[test]
fn runtime_payload_enum_and_exhaustive_match_lower_to_tagged_slots() {
    let src = "enum Event { Idle, Count(u32), Pair { left: u32; right: u32; }, } fn main() { let seed: u64 = runtime_seed(); let event: Event = Event::Pair { left: 3u32, right: 4u32 }; let value: u32 = match event { Event::Idle => 0u32, Event::Count(count) => count, Event::Pair { left: a, right: b } => a + b, }; if value != 7u32 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("payload enum match must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_generic_enum_match_substitutes_scalar_payload_types() {
    let src = "enum Outcome<T, E> { Ok(T), Err(E), } fn main() { let seed: u64 = runtime_seed(); let outcome: Outcome<u16, u32> = Outcome::Ok(9u16); let value: u16 = match outcome { Outcome::Ok(number) => number, Outcome::Err(_) => 0u16, }; if value != 9u16 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("generic enum match must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_generic_enums_cross_direct_call_and_return_abi() {
    let src = "enum Outcome<T> { None, Some(T), } fn pass(value: Outcome<u16>) -> Outcome<u16> { return value; } fn make(value: u16) -> Outcome<u16> { return Outcome::Some(value); } fn main() { let seed: u64 = runtime_seed(); let input: Outcome<u16> = make(11u16); let output: Outcome<u16> = pass(input); let value: u16 = match output { Outcome::None => 0u16, Outcome::Some(number) => number, }; if value != 11u16 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("enum call/return ABI must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_resource_enum_moves_list_through_call_return_and_match() {
    let src = "enum Payload { Empty, Values(list<u16>), } fn wrap(values: list<u16>) -> Payload { return Payload::Values(values); } fn consume(payload: Payload) -> u64 { return match payload { Payload::Empty => 0u64, Payload::Values(items) => items.len(), }; } fn main() { let seed: u64 = runtime_seed(); let values: list<u16> = [3u16, 5u16, 8u16]; let payload: Payload = wrap(values); let count: u64 = consume(payload); if count != 3u64 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("resource enum ABI must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_nested_owned_struct_moves_resources_through_call_and_return() {
    let src = "struct Inner { values: list<u16>; } struct Outer { marker: u64; inner: Inner; } fn pass(value: Outer) -> Outer { return value; } fn main() { let seed: u64 = runtime_seed(); let outer: Outer = Outer { marker: 9u64, inner: Inner { values: [2u16, 4u16, 6u16] } }; let moved: Outer = pass(outer); let count: u64 = moved.inner.values.len(); if count != 3u64 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("nested owned struct must lower")
            .last(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_recursive_resource_struct_layout_is_rejected() {
    let src = "struct Node { values: list<u16>; next: Node; } fn main() { let seed: u64 = runtime_seed(); exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("recursive resource layout must fail");
    assert!(error.message.contains("recursive resource layout"));
}

#[test]
fn runtime_resource_enum_rejects_list_reuse_after_move() {
    let src = "enum Payload { Empty, Values(list<u16>), } fn main() { let seed: u64 = runtime_seed(); let mut values: list<u16> = [3u16]; let payload: Payload = Payload::Values(values); values.push(5u16); exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("moved payload list must not be reusable");
    assert!(err.message.contains("was moved or consumed"));
}

#[test]
fn runtime_owned_utf8_string_moves_through_native_call_return_abi() {
    let src = "fn pass(text: string) -> string { return text; } fn main() { let seed: u64 = runtime_seed(); let text: string = \"λ🙂x\"; let moved: string = pass(text); if moved.len() != 7u64 { exit(1u64); } if moved.char_count() != 3u64 { exit(2u64); } let first: char = moved.char_at(0u64).unwrap_or('?'); let second: char = moved.char_at(1u64).unwrap_or('?'); if first != 'λ' { exit(3u64); } if second != '🙂' { exit(4u64); } if moved.char_at(3u64).is_some() { exit(5u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("owned UTF-8 string ABI must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_owned_string_and_map_reject_use_after_move() {
    for src in [
        "fn main() { let seed: u64 = runtime_seed(); let text: string = \"owned\"; let moved: string = text; exit(text.len() + moved.len() + (seed & 0u64)); }",
        "fn main() { let seed: u64 = runtime_seed(); let scores: map<u64, u16> = {}; let moved: map<u64, u16> = scores; exit(scores.len() + moved.len() + (seed & 0u64)); }",
    ] {
        let tokens = lex(src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let error = lower_program(&program).expect_err("moved resource must be unusable");
        assert!(
            error.message.contains("was moved or consumed"),
            "unexpected diagnostic: {}",
            error.message
        );
    }
}

#[test]
fn runtime_owned_map_supports_dynamic_keys_growth_lookup_and_remove() {
    let src = "fn pass(scores: map<u64, u16>) -> map<u64, u16> { return scores; } fn main() { let seed: u64 = runtime_seed(); let mut scores: map<u64, u16> = {}; let key: u64 = seed & 7u64; scores.set(key, 11u16); scores.set(key + 1u64, 13u16); scores.set(key + 2u64, 17u16); scores.set(key + 3u64, 19u16); scores.set(key + 4u64, 23u16); let mut moved: map<u64, u16> = pass(scores); moved.set(key, 29u16); let value: u16 = moved.get(key).unwrap_or(0u16); if value != 29u16 { exit(1u64); } if moved.len() != 5u64 { exit(2u64); } moved.remove(key + 2u64); if moved.get(key + 2u64).is_some() { exit(3u64); } if moved.len() != 4u64 { exit(4u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("owned map must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_option_and_result_share_tagged_enum_abi() {
    let src = "fn maybe(value: u16) -> Option<u16> { return Option::Some(value); } fn decide(ok: bool) -> Result<u16, u8> { if ok { return Result::Ok(13u16); } return Result::Err(2u8); } fn main() { let seed: u64 = runtime_seed(); let option: Option<u16> = maybe(12u16); if option.is_none() { exit(1u64); } let first: u16 = option.unwrap_or(0u16); let result: Result<u16, u8> = decide(true); let second: u16 = match result { Result::Ok(value) => value, Result::Err(_) => 0u16, }; if first + second != 25u16 { exit(1u64); } exit(seed & 0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        lower_program(&program)
            .expect("Option/Result tagged ABI must lower")
            .first(),
        Some(super::LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[test]
fn runtime_enum_match_rejects_non_exhaustive_coverage_deterministically() {
    let src = "enum State { Ready, Busy, Done, } fn main() { let seed: u64 = runtime_seed(); let state: State = State::Ready; let value: u64 = match state { State::Ready => 1u64, State::Busy => 2u64, }; exit(value + (seed & 0u64)); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let err = lower_program(&program).expect_err("non-exhaustive runtime match must fail");
    assert_eq!(
        err.message,
        "non-exhaustive match for enum 'State'; missing State::Done"
    );
}

#[test]
fn lower_struct_sort_hash_and_comparison() {
    let src = "struct Point { x: i32; y: i32; } fn by_x_desc(a: Point, b: Point) -> bool { return a.x > b.x; } fn main() { let mut points: [Point; 4] = [Point { x: 4i32, y: 1i32 }, Point { x: 1i32, y: 7i32 }, Point { x: 3i32, y: 9i32 }, Point { x: 2i32, y: 2i32 }]; points.sort(); print(points[0u8].x.to_str()); print(\"\\n\"); print(points[3u8].x.to_str()); print(\"\\n\"); points.sort_by(by_x_desc); print(points[0u8].x.to_str()); print(\"\\n\"); print((points[0u8] > points[1u8]).to_str()); print(\"\\n\"); print(points[0u8].hash64().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "1"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "4"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "true"))
    );
}

#[test]
fn lower_dict_with_struct_keys_set_index_and_remove() {
    let src = "struct Key { id: u64; bucket: u64; } fn main() { let mut map: dict<Key, i64> = {}; let k: Key = Key { id: 7u64, bucket: 2u64 }; map.set(k, 33i64); print(map[k].to_str()); print(\"\\n\"); map[k] = 41i64; print(map[k].to_str()); print(\"\\n\"); map.remove(k); print(map.len().to_str()); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "33"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "41"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "0"))
    );
}

#[test]
fn lower_runtime_generic_array_sort_method() {
    let src = "fn main() { let mut arr: [u64; 4] = [9u64, 1u64, 7u64, 3u64]; let mut s: u64 = runtime_seed(); arr.sort(); s = s + arr[0u8] + arr[3u8]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_full_len_guard = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::Eq,
                rhs: super::RuntimeOperand::Imm(4),
                ..
            }
        )
    });
    let has_guarded_fallback = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::GtUnsigned,
                ..
            } | super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::GtSigned,
                ..
            }
        )
    });
    let has_compare_swap = program
        .instrs
        .iter()
        .any(|instr| matches!(instr, super::RuntimeInstr::CompareSwap { .. }));
    assert!(!has_full_len_guard);
    assert!(!has_guarded_fallback);
    assert!(has_compare_swap);
}

#[test]
fn lower_runtime_generic_array_sort_after_push_has_len_guards() {
    let src = "fn main() { let mut arr: [u64; 4] = [9u64, 1u64, 7u64, 3u64]; arr.pop(); arr.push(5u64); let mut s: u64 = runtime_seed(); arr.sort(); s = s + arr[0u8] + arr[3u8]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_full_len_guard = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::Eq,
                rhs: super::RuntimeOperand::Imm(4),
                ..
            }
        )
    });
    let has_compare_swap = program
        .instrs
        .iter()
        .any(|instr| matches!(instr, super::RuntimeInstr::CompareSwap { .. }));
    assert!(has_full_len_guard);
    assert!(has_compare_swap);
}

#[test]
fn lower_runtime_generic_fixed_len_array_const_index_skips_len_checks() {
    let src = "fn main() { let mut arr: [u64; 4] = [9u64, 1u64, 7u64, 3u64]; let mut s: u64 = runtime_seed(); arr[0u8] = s; s = s + arr[0u8] + arr[1u8]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    let has_const_idx_len_check = program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::JumpIfCmpFalse {
                op: super::RuntimeCmpOp::LtUnsigned,
                lhs: super::RuntimeOperand::Imm(0 | 1),
                rhs: super::RuntimeOperand::Slot(_),
                ..
            }
        )
    });
    assert!(!has_const_idx_len_check);
}

#[test]
fn lower_radix_sort_modes_for_integer_arrays() {
    let src = "fn main() { let mut a: [u64; 8] = [91u64, 1u64, 17u64, 9u64, 3u64, 70u64, 2u64, 11u64]; a.sort_radix_unstable(); print(a[0u8].to_str()); print(\"\\n\"); print(a[7u8].to_str()); print(\"\\n\"); let mut b: [i32; 8] = [9i32, -1i32, 4i32, -7i32, 3i32, 0i32, -2i32, 6i32]; b.sort_radix_stable(); print(b[0u8].to_str()); print(\"\\n\"); print(b[7u8].to_str()); print(\"\\n\"); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "1"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "91"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "-7"))
    );
    assert!(
        lowered
            .iter()
            .any(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "9"))
    );
}

#[test]
fn lower_block_quicksort_generic_struct_modes() {
    let src = "struct Pair { x: i32; y: i32; } fn main() { let mut pairs: [Pair; 5] = [Pair { x: 4i32, y: 1i32 }, Pair { x: 1i32, y: 7i32 }, Pair { x: 3i32, y: 9i32 }, Pair { x: 2i32, y: 2i32 }, Pair { x: 5i32, y: 0i32 }]; pairs.sort_unstable(); print(pairs[0u8].x.to_str()); print(\"\\n\"); print(pairs[4u8].x.to_str()); print(\"\\n\"); pairs.sort_stable(); print(pairs[0u8].x.to_str()); print(\"\\n\"); print(pairs[4u8].x.to_str()); print(\"\\n\"); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let ones = lowered
        .iter()
        .filter(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "1"))
        .count();
    let fives = lowered
        .iter()
        .filter(|stmt| matches!(stmt, super::LoweredStmt::Print(v) if v == "5"))
        .count();
    assert!(ones >= 2);
    assert!(fives >= 2);
}

#[test]
fn lower_runtime_generic_array_radix_sort_method() {
    let src = "fn main() { let mut arr: [u64; 8] = [9u64, 1u64, 7u64, 3u64, 8u64, 2u64, 6u64, 4u64]; let mut s: u64 = runtime_seed(); arr.sort_radix_unstable(); s = s + arr[0u8] + arr[7u8]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::CompareSwap { .. }))
    );
}

#[test]
fn lower_runtime_generic_large_array_radix_emits_kernel_instr() {
    let src = "fn main() { let mut arr: [u64; 32] = [0u64, 1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64, 8u64, 9u64, 10u64, 11u64, 12u64, 13u64, 14u64, 15u64, 16u64, 17u64, 18u64, 19u64, 20u64, 21u64, 22u64, 23u64, 24u64, 25u64, 26u64, 27u64, 28u64, 29u64, 30u64, 31u64]; let mut s: u64 = runtime_seed(); arr.sort_radix_unstable(); s = s + arr[0u8] + arr[31u8]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::RadixSortFixedInt {
                bits: 64,
                signed: false,
                ..
            }
        )
    }));
}

#[test]
fn lower_runtime_generic_64_array_radix_emits_kernel_instr() {
    let src = "fn main() { let mut arr: [u64; 64] = [0u64, 1u64, 2u64, 3u64, 4u64, 5u64, 6u64, 7u64, 8u64, 9u64, 10u64, 11u64, 12u64, 13u64, 14u64, 15u64, 16u64, 17u64, 18u64, 19u64, 20u64, 21u64, 22u64, 23u64, 24u64, 25u64, 26u64, 27u64, 28u64, 29u64, 30u64, 31u64, 32u64, 33u64, 34u64, 35u64, 36u64, 37u64, 38u64, 39u64, 40u64, 41u64, 42u64, 43u64, 44u64, 45u64, 46u64, 47u64, 48u64, 49u64, 50u64, 51u64, 52u64, 53u64, 54u64, 55u64, 56u64, 57u64, 58u64, 59u64, 60u64, 61u64, 62u64, 63u64]; let mut s: u64 = runtime_seed(); arr.sort_radix_unstable(); s = s + arr[0u8] + arr[63u8]; exit(s); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    let runtime = lowered
        .first()
        .expect("missing lowered stmt for runtime generic");
    let super::LoweredStmt::RuntimeGeneric { program } = runtime else {
        panic!("expected RuntimeGeneric lowering");
    };
    assert!(program.instrs.iter().any(|instr| {
        matches!(
            instr,
            super::RuntimeInstr::RadixSortFixedInt {
                bits: 64,
                signed: false,
                ..
            }
        )
    }));
}

#[test]
fn benchmark_kernel_calls_are_rejected_with_a_migration_diagnostic() {
    for name in [
        "runtime_bloom_sbbf_insert(filter, h)",
        "runtime_bloom_sbbf_maybe(filter, h)",
        "runtime_hash_probe_grouped16(filter, h, h)",
        "runtime_join_select_adaptive(h, h)",
    ] {
        let src = format!(
            "fn main() {{ let filter: u64 = 0u64; let h: u64 = runtime_seed(); {name}; exit(0u64); }}"
        );
        let tokens = lex(&src).expect("lex failed");
        let program = parse(&tokens).expect("parse failed");
        let diagnostic = lower_program(&program).expect_err("removed call unexpectedly lowered");
        assert!(
            diagnostic.message.contains("no longer available"),
            "unexpected diagnostic for {name}: {}",
            diagnostic.message
        );
        assert!(diagnostic.message.contains("ordinary Aziky control flow"));
    }
}

#[test]
fn benchmark_sources_lower_through_generic_runtime_ir_only() {
    for src in [
        include_str!("../../../bench/affine_mix.azk"),
        include_str!("../../../bench/binary_search.azk"),
        include_str!("../../../bench/bloom_filter.azk"),
        include_str!("../../../bench/hash_join.azk"),
        include_str!("../../../bench/histogram.azk"),
        include_str!("../../../bench/packet_classifier.azk"),
        include_str!("../../../bench/prefix_scan.azk"),
        include_str!("../../../bench/ring_write.azk"),
        include_str!("../../../bench/sort_window.azk"),
        include_str!("../../../bench/stream_lcg.azk"),
    ] {
        let tokens = lex(src).expect("benchmark lex failed");
        let parsed = parse(&tokens).expect("benchmark parse failed");
        let lowered = lower_program(&parsed).expect("benchmark lowering failed");
        assert!(
            matches!(lowered.first(), Some(super::LoweredStmt::RuntimeGeneric { .. })),
            "benchmark did not lower through RuntimeGeneric"
        );
    }
}

#[cfg(any())]
#[test]
fn lower_seeded_struct_latency_loop_to_special_kernel() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut i: u64 = 0u64; let mut a: u64 = 0u64; let mut b: u64 = 0u64; let mut c: u64 = 0u64; let mut d: u64 = 0u64; while i < 100u64 { state = state * 1664525u64 + 1013904223u64; a = a + state; b = b ^ state; c = c + 1u64; d = d ^ a; a = a ^ d; i = i + 1u64; } exit(a + b + c + d); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("lower failed");
    assert!(matches!(
        lowered.first(),
        Some(super::LoweredStmt::RuntimeSeededStructLatencyLoop {
            iterations,
            mul,
            add,
            exit_with_sum
        }) if *iterations == 100 && *mul == 1_664_525 && *add == 1_013_904_223 && *exit_with_sum
    ));
}

#[test]
fn runtime_fixed_sort_pairs_has_minimal_8_network_shape() {
    let pairs = super::runtime_fixed_sort_pairs(8).expect("missing sort pairs");
    assert_eq!(pairs.len(), 19);
}

#[test]
fn runtime_fixed_sort_pairs_64_uses_compact_power2_network() {
    let pairs = super::runtime_fixed_sort_pairs(64).expect("missing sort pairs");
    assert_eq!(pairs.len(), 543);
}

#[test]
fn runtime_fixed_sort_pairs_sorts_all_8_permutations() {
    fn apply_pairs(values: &mut [u8], pairs: &[(usize, usize)]) {
        for (left, right) in pairs.iter().copied() {
            if values[left] > values[right] {
                values.swap(left, right);
            }
        }
    }

    fn next_permutation(values: &mut [u8]) -> bool {
        if values.len() < 2 {
            return false;
        }
        let mut i = values.len() - 1;
        while i > 0 && values[i - 1] >= values[i] {
            i -= 1;
        }
        if i == 0 {
            return false;
        }
        let pivot = i - 1;
        let mut j = values.len() - 1;
        while values[j] <= values[pivot] {
            j -= 1;
        }
        values.swap(pivot, j);
        values[i..].reverse();
        true
    }

    let pairs = super::runtime_fixed_sort_pairs(8).expect("missing sort pairs");
    let mut values = [0u8, 1, 2, 3, 4, 5, 6, 7];
    loop {
        let mut sample = values;
        apply_pairs(&mut sample, &pairs);
        assert!(sample.windows(2).all(|w| w[0] <= w[1]));
        if !next_permutation(&mut values) {
            break;
        }
    }
}

#[test]
fn runtime_fallback_classification_is_stable_and_ordered() {
    let span = crate::frontend::ast::Span::in_source(7, 11, 3);
    let reasons = [
        super::RuntimeFallbackReason::new(
            super::RuntimeFallbackClass::CallGraph,
            "invalid call graph",
            span,
        ),
        super::RuntimeFallbackReason::new(
            super::RuntimeFallbackClass::UnsupportedConstruct,
            "unsupported\nconstruct",
            span,
        ),
        super::RuntimeFallbackReason::new(
            super::RuntimeFallbackClass::LoweringDiagnostic,
            "typed lowering failure",
            span,
        ),
    ];
    let rendered: Vec<String> = reasons
        .iter()
        .map(super::RuntimeFallbackReason::render)
        .collect();
    assert_eq!(
        rendered,
        vec![
            "code=AZL001 class=call-graph location=7:11 detail=invalid call graph",
            "code=AZL002 class=unsupported-construct location=7:11 detail=unsupported construct",
            "code=AZL003 class=lowering-diagnostic location=7:11 detail=typed lowering failure",
        ]
    );
}

#[test]
fn native_thread_and_channel_owners_lower_to_linear_runtime_operations() {
    let src = "fn worker(sender: Sender<u64>, value: u64) -> u64 { sender.send(value); sender.close(); return value; } fn main() { let channel: Channel<u64> = Channel::bounded(2u64); let sender: Sender<u64> = channel.sender(); let receiver: Receiver<u64> = channel.receiver(); let thread: Thread = Thread::spawn(worker, sender, 42u64); let value: u64 = receiver.recv(); receiver.close(); let status: u64 = thread.join(); exit(value + status); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let lowered = lower_program(&program).expect("thread/channel program must lower");
    let super::LoweredStmt::RuntimeGeneric { program } = &lowered[0] else {
        panic!("expected runtime generic lowering");
    };
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::ThreadSpawn { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::ThreadJoin { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::ChannelSend { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::ChannelRecv { .. }))
    );
    assert!(
        program
            .instrs
            .iter()
            .any(|instr| matches!(instr, super::RuntimeInstr::ChannelDestroy { .. }))
    );
}

#[test]
fn thread_and_channel_linearity_diagnostics_are_stable() {
    let duplicate = "fn main() { let channel: Channel<u64> = Channel::bounded(2u64); let first: Sender<u64> = channel.sender(); let second: Sender<u64> = channel.sender(); exit(0u64); }";
    let tokens = lex(duplicate).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("duplicate sender must fail");
    assert!(
        error.message.contains("extracted exactly once"),
        "{error:?}"
    );

    let moved = "fn worker(sender: Sender<u64>) -> u64 { sender.close(); return 0u64; } fn main() { let channel: Channel<u64> = Channel::bounded(2u64); let sender: Sender<u64> = channel.sender(); let thread: Thread = Thread::spawn(worker, sender); sender.send(1u64); let status: u64 = thread.join(); exit(status); }";
    let tokens = lex(moved).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let error = lower_program(&program).expect_err("moved endpoint reuse must fail");
    assert!(error.message.contains("moved or consumed"), "{error:?}");
}
