use std::fmt;
use std::path::Path;

pub(crate) fn display_path(path: &Path) -> String {
    let display = path.to_string_lossy();
    if let Some(path) = display.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{path}")
    } else if let Some(path) = display.strip_prefix(r"\\?\") {
        path.to_string()
    } else {
        display.into_owned()
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub source_id: usize,
    pub trace: Vec<String>,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            message: message.into(),
            line,
            column,
            source_id: 0,
            trace: Vec::new(),
        }
    }

    pub fn at_span(
        message: impl Into<String>,
        span: impl Into<crate::frontend::ast::Span>,
    ) -> Self {
        let span = span.into();
        Self::new(message, span.line, span.column).with_source(span.source_id)
    }

    pub fn with_source(mut self, source_id: usize) -> Self {
        self.source_id = source_id;
        self
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.trace.push(context.into());
        self
    }

    pub fn render(&self, source: &str, path: &Path) -> String {
        let mut out = String::new();
        let line_text = source
            .lines()
            .nth(self.line.saturating_sub(1))
            .unwrap_or("");
        let caret_pad = if self.column > 0 { self.column - 1 } else { 0 };

        out.push_str(&format!(
            "{} at {}:{}\n",
            self.message, self.line, self.column
        ));
        out.push_str(&format!(" --> {}\n", display_path(path)));
        out.push_str(line_text);
        out.push('\n');
        out.push_str(&" ".repeat(caret_pad));
        out.push('^');

        if !self.trace.is_empty() {
            out.push_str("\nstack:\n");
            for (index, context) in self.trace.iter().rev().enumerate() {
                out.push_str(&format!("  {}: {}\n", index + 1, context));
            }
        }

        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}:{}", self.message, self.line, self.column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_paths_hide_windows_verbatim_drive_prefixes() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\work\aziky\main.azk")),
            r"C:\work\aziky\main.azk"
        );
    }

    #[test]
    fn diagnostic_paths_restore_windows_unc_prefixes() {
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\main.azk")),
            r"\\server\share\main.azk"
        );
    }
}
