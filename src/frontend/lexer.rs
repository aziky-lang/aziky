use crate::frontend::diagnostics::Diagnostic;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Fn,
    Struct,
    Enum,
    Trait,
    Impl,
    Embed,
    Pub,
    As,
    Mod,
    Use,
    Let,
    Mut,
    If,
    Else,
    Match,
    While,
    Loop,
    For,
    ParFor,
    Foreach,
    In,
    Break,
    Continue,
    Return,
    Assert,
    Panic,
    True,
    False,
    Ident(String),
    Print,
    Exit,
    BenchLoop,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semicolon,
    Colon,
    ColonColon,
    Comma,
    Dot,
    DotDot,
    Equal,
    FatArrow,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Ampersand,
    AmpAmp,
    Pipe,
    PipePipe,
    Caret,
    ShiftLeft,
    ShiftRight,
    Bang,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Number(String),
    String(String),
    Char(char),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
    pub source_id: usize,
}

pub fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    let mut line = 1;
    let mut column = 1;

    while let Some(ch) = chars.peek().copied() {
        match ch {
            ' ' | '\t' | '\r' => {
                chars.next();
                column += 1;
            }
            '\n' => {
                chars.next();
                line += 1;
                column = 1;
            }
            '(' => {
                push_token(&mut tokens, TokenKind::LParen, line, column);
                chars.next();
                column += 1;
            }
            ')' => {
                push_token(&mut tokens, TokenKind::RParen, line, column);
                chars.next();
                column += 1;
            }
            '{' => {
                push_token(&mut tokens, TokenKind::LBrace, line, column);
                chars.next();
                column += 1;
            }
            '}' => {
                push_token(&mut tokens, TokenKind::RBrace, line, column);
                chars.next();
                column += 1;
            }
            '[' => {
                push_token(&mut tokens, TokenKind::LBracket, line, column);
                chars.next();
                column += 1;
            }
            ']' => {
                push_token(&mut tokens, TokenKind::RBracket, line, column);
                chars.next();
                column += 1;
            }
            ';' => {
                push_token(&mut tokens, TokenKind::Semicolon, line, column);
                chars.next();
                column += 1;
            }
            ':' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some(':')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::ColonColon, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Colon, line, column - 1);
                }
            }
            ',' => {
                push_token(&mut tokens, TokenKind::Comma, line, column);
                chars.next();
                column += 1;
            }
            '.' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('.')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::DotDot, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Dot, line, column - 1);
                }
            }
            '=' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('=')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::EqualEqual, line, column - 2);
                } else if matches!(chars.peek(), Some('>')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::FatArrow, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Equal, line, column - 1);
                }
            }
            '!' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('=')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::BangEqual, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Bang, line, column - 1);
                }
            }
            '<' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('=')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::LessEqual, line, column - 2);
                } else if matches!(chars.peek(), Some('<')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::ShiftLeft, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Less, line, column - 1);
                }
            }
            '>' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('=')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::GreaterEqual, line, column - 2);
                } else if matches!(chars.peek(), Some('>')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::ShiftRight, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Greater, line, column - 1);
                }
            }
            '+' => {
                push_token(&mut tokens, TokenKind::Plus, line, column);
                chars.next();
                column += 1;
            }
            '-' => {
                push_token(&mut tokens, TokenKind::Minus, line, column);
                chars.next();
                column += 1;
            }
            '*' => {
                push_token(&mut tokens, TokenKind::Star, line, column);
                chars.next();
                column += 1;
            }
            '/' => {
                chars.next();
                let start_line = line;
                let start_col = column;
                column += 1;
                if matches!(chars.peek(), Some('/')) {
                    // Line comment: consume until newline or EOF.
                    chars.next();
                    column += 1;
                    while let Some(next) = chars.peek().copied() {
                        if next == '\n' {
                            break;
                        }
                        chars.next();
                        column += 1;
                    }
                    continue;
                }
                if matches!(chars.peek(), Some('*')) {
                    // Block comment: supports nesting for deterministic parsing behavior.
                    chars.next();
                    column += 1;
                    let mut depth = 1usize;
                    while let Some(next) = chars.next() {
                        match next {
                            '\n' => {
                                line += 1;
                                column = 1;
                            }
                            '/' => {
                                column += 1;
                                if matches!(chars.peek(), Some('*')) {
                                    chars.next();
                                    column += 1;
                                    depth += 1;
                                }
                            }
                            '*' => {
                                column += 1;
                                if matches!(chars.peek(), Some('/')) {
                                    chars.next();
                                    column += 1;
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                            }
                            _ => {
                                column += 1;
                            }
                        }
                    }
                    if depth != 0 {
                        return Err(Diagnostic::new(
                            "unterminated block comment",
                            start_line,
                            start_col,
                        ));
                    }
                    continue;
                }
                push_token(&mut tokens, TokenKind::Slash, start_line, start_col);
            }
            '%' => {
                push_token(&mut tokens, TokenKind::Percent, line, column);
                chars.next();
                column += 1;
            }
            '&' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('&')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::AmpAmp, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Ampersand, line, column - 1);
                }
            }
            '|' => {
                chars.next();
                column += 1;
                if matches!(chars.peek(), Some('|')) {
                    chars.next();
                    column += 1;
                    push_token(&mut tokens, TokenKind::PipePipe, line, column - 2);
                } else {
                    push_token(&mut tokens, TokenKind::Pipe, line, column - 1);
                }
            }
            '^' => {
                push_token(&mut tokens, TokenKind::Caret, line, column);
                chars.next();
                column += 1;
            }
            '"' => {
                let start_line = line;
                let start_col = column;
                chars.next();
                column += 1;
                let mut value = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    match next {
                        '"' => {
                            column += 1;
                            closed = true;
                            break;
                        }
                        '\\' => {
                            column += 1;
                            let escaped = chars.next().ok_or_else(|| {
                                Diagnostic::new(
                                    "unterminated escape sequence",
                                    start_line,
                                    start_col,
                                )
                            })?;
                            column += 1;
                            match escaped {
                                'n' => value.push('\n'),
                                't' => value.push('\t'),
                                '"' => value.push('"'),
                                '\\' => value.push('\\'),
                                other => {
                                    return Err(Diagnostic::new(
                                        format!("unsupported escape: \\{other}"),
                                        start_line,
                                        start_col,
                                    ));
                                }
                            }
                        }
                        '\n' => {
                            return Err(Diagnostic::new(
                                "unterminated string literal",
                                start_line,
                                start_col,
                            ));
                        }
                        other => {
                            column += 1;
                            value.push(other);
                        }
                    }
                }

                if !closed {
                    return Err(Diagnostic::new(
                        "unterminated string literal",
                        start_line,
                        start_col,
                    ));
                }

                push_token(&mut tokens, TokenKind::String(value), start_line, start_col);
            }
            '\'' => {
                let start_line = line;
                let start_col = column;
                chars.next();
                column += 1;
                let next = chars.next().ok_or_else(|| {
                    Diagnostic::new("unterminated char literal", start_line, start_col)
                })?;
                column += 1;
                let value = match next {
                    '\'' => {
                        return Err(Diagnostic::new(
                            "char literal cannot be empty",
                            start_line,
                            start_col,
                        ));
                    }
                    '\n' => {
                        return Err(Diagnostic::new(
                            "unterminated char literal",
                            start_line,
                            start_col,
                        ));
                    }
                    '\\' => {
                        let escaped = chars.next().ok_or_else(|| {
                            Diagnostic::new(
                                "unterminated escape sequence in char literal",
                                start_line,
                                start_col,
                            )
                        })?;
                        column += 1;
                        match escaped {
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            '0' => '\0',
                            '\'' => '\'',
                            '"' => '"',
                            '\\' => '\\',
                            other => {
                                return Err(Diagnostic::new(
                                    format!("unsupported char escape: \\{other}"),
                                    start_line,
                                    start_col,
                                ));
                            }
                        }
                    }
                    value => value,
                };
                match chars.next() {
                    Some('\'') => {
                        column += 1;
                        push_token(&mut tokens, TokenKind::Char(value), start_line, start_col);
                    }
                    Some('\n') | None => {
                        return Err(Diagnostic::new(
                            "unterminated char literal",
                            start_line,
                            start_col,
                        ));
                    }
                    Some(_) => {
                        return Err(Diagnostic::new(
                            "char literal must contain exactly one Unicode scalar value",
                            start_line,
                            start_col,
                        ));
                    }
                }
            }
            '0'..='9' => {
                let start_line = line;
                let start_col = column;
                let mut literal = String::new();
                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_digit() {
                        literal.push(next);
                        chars.next();
                        column += 1;
                    } else {
                        break;
                    }
                }

                if let Some('.') = chars.peek().copied() {
                    let mut lookahead = chars.clone();
                    lookahead.next();
                    if let Some(next) = lookahead.peek().copied() {
                        if next.is_ascii_digit() {
                            literal.push('.');
                            chars.next();
                            column += 1;
                            while let Some(next) = chars.peek().copied() {
                                if next.is_ascii_digit() {
                                    literal.push(next);
                                    chars.next();
                                    column += 1;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }

                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_alphanumeric() {
                        literal.push(next);
                        chars.next();
                        column += 1;
                    } else {
                        break;
                    }
                }

                push_token(
                    &mut tokens,
                    TokenKind::Number(literal),
                    start_line,
                    start_col,
                );
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start_line = line;
                let start_col = column;
                let mut ident = String::new();
                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_alphanumeric() || next == '_' {
                        ident.push(next);
                        chars.next();
                        column += 1;
                    } else {
                        break;
                    }
                }

                let kind = match ident.as_str() {
                    "fn" => TokenKind::Fn,
                    "struct" => TokenKind::Struct,
                    "enum" => TokenKind::Enum,
                    "trait" => TokenKind::Trait,
                    "impl" => TokenKind::Impl,
                    "embed" => TokenKind::Embed,
                    "pub" => TokenKind::Pub,
                    "as" => TokenKind::As,
                    "mod" => TokenKind::Mod,
                    "use" => TokenKind::Use,
                    "let" => TokenKind::Let,
                    "mut" => TokenKind::Mut,
                    "if" => TokenKind::If,
                    "else" => TokenKind::Else,
                    "match" => TokenKind::Match,
                    "while" => TokenKind::While,
                    "loop" => TokenKind::Loop,
                    "for" => TokenKind::For,
                    "parfor" => TokenKind::ParFor,
                    "foreach" => TokenKind::Foreach,
                    "in" => TokenKind::In,
                    "break" => TokenKind::Break,
                    "continue" => TokenKind::Continue,
                    "return" => TokenKind::Return,
                    "assert" => TokenKind::Assert,
                    "panic" => TokenKind::Panic,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "print" => TokenKind::Print,
                    "exit" => TokenKind::Exit,
                    "benchloop" => TokenKind::BenchLoop,
                    _ => TokenKind::Ident(ident),
                };
                push_token(&mut tokens, kind, start_line, start_col);
            }
            other => {
                return Err(Diagnostic::new(
                    format!("unexpected character: {other}"),
                    line,
                    column,
                ));
            }
        }
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        line,
        column,
        source_id: 0,
    });

    Ok(tokens)
}

fn push_token(tokens: &mut Vec<Token>, kind: TokenKind, line: usize, column: usize) {
    tokens.push(Token {
        kind,
        line,
        column,
        source_id: 0,
    });
}

#[cfg(test)]
#[path = "lexer/tests.rs"]
mod tests;
