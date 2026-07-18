pub mod ast;
pub mod diagnostics;
pub mod lexer;
pub mod optimizer;
pub mod parser;
pub mod semantics;

use crate::frontend::ast::Program;

pub use diagnostics::Diagnostic;
pub use optimizer::optimize_semantics_ir;
pub use semantics::{LoweredStmt, lower_program};

pub fn parse_program(source: &str) -> Result<Program, Diagnostic> {
    let tokens = lexer::lex(source)?;
    parser::parse(&tokens)
}

pub fn parse_program_in_source(source: &str, source_id: usize) -> Result<Program, Diagnostic> {
    let mut tokens = lexer::lex(source).map_err(|diagnostic| diagnostic.with_source(source_id))?;
    for token in &mut tokens {
        token.source_id = source_id;
    }
    parser::parse(&tokens)
}
