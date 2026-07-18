use super::*;

#[test]
fn lex_simple_program() {
    let src = "trait T { fn f(x: u8) -> u8; } enum State { Ready, Done } struct Base { id: u8; } struct Foo { embed Base; x: u8; } impl T for Foo { fn f(x: u8) -> u8 { return x; } } fn main() { let msg: string = \"hi\" + \"!\"; let arr: [u8; 2] = [1u8, 2u8]; let m: dict<string, u8> = {\"a\": 1u8}; let z: u8 = (1u8 % 2u8) | (3u8 ^ 1u8); let b: bool = !false && true || false; let s2: u8 = 8u8 >> 1u8 << 1u8; parfor t in 0u8..2u8 { print(t.to_str()); } foreach k in m { print(k); } for i in 0u8..2u8 { if true { continue; } } loop { break; } assert(true); benchloop(128u64); let s = State::Ready; let p = Foo { id: 1u8, x: 1u8 }; panic(\"stop\"); print(p.x); exit(0u8); }";
    let tokens = lex(src).expect("lex failed");
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Trait)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Impl)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Embed)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Enum)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Struct)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Fn)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Let)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::LBracket)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Dot)));
    assert!(
        tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::ColonColon))
    );
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::For)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::ParFor)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Foreach)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Loop)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Break)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Continue)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::DotDot)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Percent)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Pipe)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Caret)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Bang)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::AmpAmp)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::PipePipe)));
    assert!(
        tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::ShiftLeft))
    );
    assert!(
        tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::ShiftRight))
    );
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Assert)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Panic)));
    assert!(
        tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::BenchLoop))
    );
}

#[test]
fn lex_match_and_fat_arrow() {
    let tokens = lex("match value { State::Ready => 1u8, _ => 0u8 }").expect("lex failed");
    assert!(
        tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Match))
    );
    assert!(
        tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::FatArrow))
    );
}

#[test]
fn lex_public_visibility() {
    let tokens = lex("pub use math::add as sum;").expect("lex failed");
    assert!(
        tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Pub))
    );
    assert!(
        tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::As))
    );
}

#[test]
fn lex_skips_line_comments() {
    let src = "fn main() { // skip this\n let x: u8 = 1u8; }";
    let tokens = lex(src).expect("lex failed");
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Fn)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Let)));
    assert!(
        !tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::Ident(name) if name == "skip"))
    );
}

#[test]
fn lex_skips_block_comments() {
    let src = "fn main() { /* remove me */ let x: u8 = 1u8; }";
    let tokens = lex(src).expect("lex failed");
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Fn)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Let)));
    assert!(
        !tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::Ident(name) if name == "remove"))
    );
}

#[test]
fn lex_skips_nested_block_comments() {
    let src = "fn main() { /* outer /* inner */ still outer */ let x: u8 = 1u8; }";
    let tokens = lex(src).expect("lex failed");
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Fn)));
    assert!(tokens.iter().any(|t| matches!(t.kind, TokenKind::Let)));
    assert!(
        !tokens
            .iter()
            .any(|t| matches!(&t.kind, TokenKind::Ident(name) if name == "inner"))
    );
}

#[test]
fn lex_reports_unterminated_block_comment() {
    let src = "fn main() { /* never closed";
    let err = lex(src).expect_err("expected unterminated block comment error");
    assert!(err.message.contains("unterminated block comment"));
}

#[test]
fn lexes_unicode_and_escaped_char_literals() {
    let tokens = lex("'A' 'λ' '\\n' '\\'' '\\\\' '\\0'").expect("lex failed");
    let values: Vec<char> = tokens
        .iter()
        .filter_map(|token| match token.kind {
            TokenKind::Char(value) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(values, vec!['A', 'λ', '\n', '\'', '\\', '\0']);
}

#[test]
fn rejects_empty_and_multi_scalar_char_literals() {
    let empty = lex("''").expect_err("expected empty char error");
    assert!(empty.message.contains("cannot be empty"));

    let multiple = lex("'ab'").expect_err("expected multi-char error");
    assert!(multiple.message.contains("exactly one Unicode scalar"));
}

#[test]
fn rejects_unterminated_and_unknown_char_escapes() {
    let unterminated = lex("'a").expect_err("expected unterminated char error");
    assert!(unterminated.message.contains("unterminated char literal"));

    let unknown = lex("'\\x'").expect_err("expected unsupported escape error");
    assert!(unknown.message.contains("unsupported char escape"));
}
