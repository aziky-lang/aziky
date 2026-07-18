use super::*;
use crate::frontend::lexer::lex;

#[test]
fn parse_program_with_struct_and_main() {
    let src = "struct Foo { x: u8; } fn main() { exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 2);
}

#[test]
fn parse_array_literal_and_index() {
    let src = "fn main() { let arr: [u8; 2] = [1u8, 2u8]; let x = arr[1u8]; exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_struct_literal_and_field() {
    let src = "struct P { x: i32; } fn main() { let p = P { x: 1i32 }; let y = p.x; exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 2);
}

#[test]
fn parse_method_calls() {
    let src = "fn main() { let s = \"42\"; let n = s.to_i32(); let t = n.to_str(); exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_expression_method_call_with_args() {
    let src = "struct P { x: i32; } fn shift(self_ref: &P, d: i32) -> i32 { return self_ref.x + d; } fn main() { let p: P = P { x: 1i32 }; let y: i32 = p.shift(2i32); exit(y); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 3);
}

#[test]
fn parse_expression_call() {
    let src = "fn main() { let seed: u64 = runtime_seed(); exit(seed); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_extended_loops() {
    let src = "fn main() { parfor p in 0u8..2u8 { print(p.to_str()); } for i in 0u8..4u8 { if i == 2u8 { continue; } } loop { break; } exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_parfor_reduction() {
    let src = "fn main() { let mut total: i64 = 0i64; parfor i in 1i64..10i64 reduce sum into total { i }; exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_benchloop_stmt() {
    let src = "fn main() { benchloop(1000000u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_enum_foreach_dict_and_errors() {
    let src = "enum State { Ready, Done } fn main() { let m: dict<string, i32> = {\"x\": 1i32, \"y\": 2i32}; foreach k in m { print(k); } let s = State::Ready; if true { assert(true, \"ok\"); } else if false { panic(\"bad\"); } exit(0); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 2);
}

#[test]
fn parse_enum_unit_tuple_and_named_payloads() {
    let src = "enum Message { Quit, Move(i32, i32), Write { text: string; code: u32; }, } fn main() { let quit: Message = Message::Quit; let moved: Message = Message::Move(3i32, 4i32); let written: Message = Message::Write { text: \"ready\", code: 7u32 }; exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let Item::Enum(def) = &program.items[0] else {
        panic!("expected enum item");
    };
    assert!(matches!(
        def.variants[0].payload,
        EnumVariantPayloadDef::Unit
    ));
    let EnumVariantPayloadDef::Tuple(fields) = &def.variants[1].payload else {
        panic!("expected tuple payload");
    };
    assert_eq!(fields.len(), 2);
    let EnumVariantPayloadDef::Named(fields) = &def.variants[2].payload else {
        panic!("expected named payload");
    };
    assert_eq!(fields.len(), 2);
}

#[test]
fn parse_exhaustive_enum_match_expression() {
    let src = "enum Message { Quit, Move(i32, i32), Write { text: string; code: u32; }, } fn describe(message: Message) -> string { return match message { Message::Quit => \"quit\", Message::Move(x, _) => x.to_str(), Message::Write { text, code: _ } => text, }; } fn main() { exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let Item::Function(function) = &program.items[1] else {
        panic!("expected function item");
    };
    let Stmt::Return {
        expr: Some(Expr::Match { arms, .. }),
        ..
    } = &function.body[0]
    else {
        panic!("expected match return expression");
    };
    assert_eq!(arms.len(), 3);
    assert!(matches!(arms[0].pattern, MatchPattern::EnumUnit { .. }));
    assert!(matches!(arms[1].pattern, MatchPattern::EnumTuple { .. }));
    assert!(matches!(arms[2].pattern, MatchPattern::EnumNamed { .. }));
}

#[test]
fn parse_generic_enum_and_applied_types() {
    let src = "enum Outcome<T, E> { Ok(T), Err(E), } fn pass(value: Outcome<i32, string>) -> Option<i32> { return Option::Some(1i32); } fn main() { exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let Item::Enum(def) = &program.items[0] else {
        panic!("expected generic enum");
    };
    assert_eq!(def.type_params, vec!["T", "E"]);
    let Item::Function(function) = &program.items[1] else {
        panic!("expected function");
    };
    assert_eq!(function.params[0].ty.display(), "Outcome<i32, string>");
    assert_eq!(
        function
            .return_type
            .as_ref()
            .expect("return type")
            .display(),
        "Option<i32>"
    );
}

#[test]
fn parse_module_and_selective_use_declarations() {
    let tokens =
        lex("mod math; pub use math::add as sum; pub fn helper() { } fn main() { exit(0u64); }")
            .expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(
        &program.items[0],
        Item::Module(ModuleDecl { name, .. }) if name == "math"
    ));
    assert!(matches!(
        &program.items[1],
        Item::Use(UseDecl { module, name, alias: Some(alias), public: true, .. })
            if module == "math" && name == "add" && alias == "sum"
    ));
    assert!(matches!(&program.items[2], Item::Function(function) if function.public));
}

#[test]
fn parse_public_top_level_declarations() {
    let tokens = lex("pub struct Item { id: u64; } pub enum State { Ready, } pub trait Named { fn name(self: &Self) -> string; } pub fn helper() { } fn main() { exit(0u64); }")
        .expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert!(matches!(&program.items[0], Item::Struct(def) if def.public));
    assert!(matches!(&program.items[1], Item::Enum(def) if def.public));
    assert!(matches!(&program.items[2], Item::Trait(def) if def.public));
    assert!(matches!(&program.items[3], Item::Function(def) if def.public));
}

#[test]
fn reject_public_impl_block() {
    let tokens = lex("struct Item { id: u64; } pub impl Item { fn id(self: &Self) -> u64 { return self.id; } } fn main() { exit(0u64); }")
        .expect("lex failed");
    let error = parse(&tokens).expect_err("public impl must be rejected");
    assert!(
        error
            .message
            .contains("'pub' is not allowed on impl blocks")
    );
}

#[test]
fn parse_owned_list_and_map_types() {
    let tokens =
        lex("fn consume(values: list<i32>, names: map<string, u64>) { } fn main() { exit(0u64); }")
            .expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    let Item::Function(function) = &program.items[0] else {
        panic!("expected function");
    };
    assert_eq!(function.params[0].ty.display(), "list<i32>");
    assert_eq!(function.params[1].ty.display(), "map<string, u64>");
}

#[test]
fn parse_if_with_ident_condition_and_block() {
    let src = "fn main() { let mut a: u64 = 1u64; let b: u64 = 2u64; if a < b { a = a + 1u64; } exit(a); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_index_assignment_stmt() {
    let src = "fn main() { let mut arr: [u64; 2] = [1u64, 2u64]; arr[1u64] = 5u64; exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_field_assignment_stmt() {
    let src =
        "struct P { x: u64; } fn main() { let mut p: P = P { x: 1u64 }; p.x = 7u64; exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 2);
}

#[test]
fn parse_method_call_stmt_with_args() {
    let src = "fn main() { let mut arr: [u64; 4] = [1u64, 2u64, 3u64, 4u64]; arr.push(5u64); arr.pop(); exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}

#[test]
fn parse_trait_impl_and_embed_struct() {
    let src = "trait Calib { fn calibrate(self_ref: &Module) -> u64; } struct Sensor { id: u64; value: u64; } struct Module { embed Sensor; unit: u64; } impl Calib for Module { fn calibrate(self_ref: &Module) -> u64 { return self_ref.value + self_ref.unit; } } fn main() { let m: Module = Module { id: 1u64, value: 2u64, unit: 3u64 }; exit(m.calibrate()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 5);
}

#[test]
fn parse_inherent_impl_and_associated_call() {
    let src = "struct Point { x: i32; } impl Point { fn new(x: i32) -> Self { return Self { x: x }; } fn get(self: &Self) -> i32 { return self.x; } } fn main() { let point: Point = Point::new(7i32); exit(point.get()); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 3);
    let Item::InherentImpl(impl_def) = &program.items[1] else {
        panic!("expected inherent impl item");
    };
    assert_eq!(impl_def.for_type, "Point");
    assert_eq!(impl_def.methods.len(), 2);
}

#[test]
fn parse_bitwise_logical_shift_and_mod_ops() {
    let src = "fn main() { let a: u64 = (9u64 % 4u64) | (8u64 >> 1u64); let b: u64 = (a << 2u64) ^ 3u64; let c: bool = !false && true || false; let d: usize = 12usize; let e: isize = -3isize; exit(0u64); }";
    let tokens = lex(src).expect("lex failed");
    let program = parse(&tokens).expect("parse failed");
    assert_eq!(program.items.len(), 1);
}
