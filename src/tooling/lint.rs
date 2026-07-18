use std::path::{Path, PathBuf};

use crate::frontend::ast::{EnumVariantPayloadDef, Function, Item, Span};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LintDiagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub code: &'static str,
    pub message: String,
}

impl LintDiagnostic {
    fn new(path: &Path, span: Span, code: &'static str, message: String) -> Self {
        Self {
            path: path.to_path_buf(),
            line: span.line,
            column: span.column,
            code,
            message,
        }
    }

    pub fn render(&self, display_path: &str) -> String {
        format!(
            "{display_path}:{}:{}: warning[{}]: {}",
            self.line, self.column, self.code, self.message
        )
    }
}

pub fn lint_source(path: &Path, source: &str) -> Result<Vec<LintDiagnostic>, String> {
    let program = crate::frontend::parse_program(source)
        .map_err(|diagnostic| diagnostic.with_context("lint::parse").render(source, path))?;
    let mut diagnostics = Vec::new();
    lint_text(path, source, &mut diagnostics);
    for item in &program.items {
        match item {
            Item::Function(function) => lint_function(path, function, &mut diagnostics),
            Item::Struct(definition) => {
                lint_upper_camel(
                    path,
                    &definition.name,
                    definition.span,
                    "type",
                    &mut diagnostics,
                );
                for field in &definition.fields {
                    lint_snake(path, &field.name, field.span, "field", &mut diagnostics);
                }
            }
            Item::Enum(definition) => {
                lint_upper_camel(
                    path,
                    &definition.name,
                    definition.span,
                    "type",
                    &mut diagnostics,
                );
                for variant in &definition.variants {
                    lint_upper_camel(
                        path,
                        &variant.name,
                        variant.span,
                        "variant",
                        &mut diagnostics,
                    );
                    if let EnumVariantPayloadDef::Named(fields) = &variant.payload {
                        for field in fields {
                            lint_snake(path, &field.name, field.span, "field", &mut diagnostics);
                        }
                    }
                }
            }
            Item::Trait(definition) => {
                lint_upper_camel(
                    path,
                    &definition.name,
                    definition.span,
                    "trait",
                    &mut diagnostics,
                );
                for method in &definition.methods {
                    lint_snake(
                        path,
                        &method.name,
                        method.span,
                        "function",
                        &mut diagnostics,
                    );
                    for parameter in &method.params {
                        lint_snake(
                            path,
                            &parameter.name,
                            parameter.span,
                            "parameter",
                            &mut diagnostics,
                        );
                    }
                }
            }
            Item::Impl(definition) => {
                for method in &definition.methods {
                    lint_function(path, method, &mut diagnostics);
                }
            }
            Item::InherentImpl(definition) => {
                for method in &definition.methods {
                    lint_function(path, method, &mut diagnostics);
                }
            }
            Item::Module(declaration) => lint_snake(
                path,
                &declaration.name,
                declaration.span,
                "module",
                &mut diagnostics,
            ),
            Item::Use(_) => {}
        }
    }
    diagnostics.sort();
    Ok(diagnostics)
}

fn lint_text(path: &Path, source: &str, diagnostics: &mut Vec<LintDiagnostic>) {
    let mut scan_line = 1usize;
    let mut scan_column = 1usize;
    for ch in source.chars() {
        if ch == '\r' {
            diagnostics.push(LintDiagnostic::new(
                path,
                Span::new(scan_line, scan_column),
                "AZK-L005",
                "carriage-return newline is not portable; run `aziky fmt`".to_string(),
            ));
        }
        if ch == '\n' {
            scan_line += 1;
            scan_column = 1;
        } else {
            scan_column += 1;
        }
    }
    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        if line.ends_with(' ') || line.ends_with('\t') {
            diagnostics.push(LintDiagnostic::new(
                path,
                Span::new(line_number, line.trim_end().chars().count() + 1),
                "AZK-L001",
                "trailing whitespace; run `aziky fmt`".to_string(),
            ));
        }
        let indentation = line
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .collect::<String>();
        if let Some(column) = indentation.chars().position(|ch| ch == '\t') {
            diagnostics.push(LintDiagnostic::new(
                path,
                Span::new(line_number, column + 1),
                "AZK-L002",
                "tab indentation is not portable; use four spaces".to_string(),
            ));
        }
        let width = line.chars().count();
        if width > 120 {
            diagnostics.push(LintDiagnostic::new(
                path,
                Span::new(line_number, 121),
                "AZK-L003",
                format!("line is {width} columns; the baseline limit is 120"),
            ));
        }
    }
    if !source.is_empty() && !source.ends_with('\n') {
        let line = source.lines().count().max(1);
        let column = source
            .lines()
            .last()
            .map_or(1, |line| line.chars().count() + 1);
        diagnostics.push(LintDiagnostic::new(
            path,
            Span::new(line, column),
            "AZK-L004",
            "source must end with a newline; run `aziky fmt`".to_string(),
        ));
    }
}

fn lint_function(path: &Path, function: &Function, diagnostics: &mut Vec<LintDiagnostic>) {
    lint_snake(path, &function.name, function.span, "function", diagnostics);
    for parameter in &function.params {
        lint_snake(
            path,
            &parameter.name,
            parameter.span,
            "parameter",
            diagnostics,
        );
    }
}

fn lint_snake(
    path: &Path,
    name: &str,
    span: Span,
    kind: &str,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if !is_snake_case(name) {
        diagnostics.push(LintDiagnostic::new(
            path,
            span,
            "AZK-L100",
            format!("{kind} '{name}' should use snake_case"),
        ));
    }
}

fn lint_upper_camel(
    path: &Path,
    name: &str,
    span: Span,
    kind: &str,
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    if !is_upper_camel_case(name) {
        diagnostics.push(LintDiagnostic::new(
            path,
            span,
            "AZK-L101",
            format!("{kind} '{name}' should use UpperCamelCase"),
        ));
    }
}

fn is_snake_case(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('_')
        && !value.ends_with('_')
        && !value.contains("__")
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn is_upper_camel_case(value: &str) -> bool {
    value
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
        && value.chars().all(|ch| ch.is_ascii_alphanumeric())
        && !value.contains('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_diagnostics_are_sorted_and_stable() {
        let source = "struct bad_name { BadField: u64, }  \r\nfn BadFunction(BadParam: u64) {\n}\n";
        let diagnostics = lint_source(Path::new("sample.azk"), source).expect("lint source");
        let codes: Vec<_> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert!(codes.contains(&"AZK-L001"));
        assert!(codes.contains(&"AZK-L005"));
        assert!(codes.contains(&"AZK-L100"));
        assert!(codes.contains(&"AZK-L101"));
        assert!(diagnostics.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
