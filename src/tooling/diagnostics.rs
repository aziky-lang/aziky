use std::path::Path;

/// Stable, editor-facing representation of a compiler diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineDiagnostic {
    pub severity: &'static str,
    pub code: Option<String>,
    pub message: String,
    pub path: String,
    pub line: usize,
    pub column: usize,
}

impl MachineDiagnostic {
    pub fn from_rendered(rendered: &str, fallback_path: &Path) -> Self {
        let first_line = rendered.lines().next().unwrap_or(rendered).trim();
        let (message, line, column) =
            parse_location(first_line).unwrap_or_else(|| (first_line.to_string(), 1, 1));
        let path = rendered
            .lines()
            .find_map(|line| line.trim().strip_prefix("--> "))
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| fallback_path.to_string_lossy().into_owned());
        Self {
            severity: "error",
            code: None,
            message,
            path,
            line: line.max(1),
            column: column.max(1),
        }
    }

    pub fn collection_json(status: &str, diagnostics: &[Self]) -> String {
        let diagnostics = diagnostics
            .iter()
            .map(Self::to_json)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"schema\":\"aziky-diagnostics-v1\",\"status\":\"{}\",\"diagnostics\":[{}]}}",
            json_escape(status),
            diagnostics
        )
    }

    fn to_json(&self) -> String {
        let code = self
            .code
            .as_ref()
            .map(|code| format!(",\"code\":\"{}\"", json_escape(code)))
            .unwrap_or_default();
        format!(
            "{{\"severity\":\"{}\"{},\"message\":\"{}\",\"path\":\"{}\",\"line\":{},\"column\":{}}}",
            json_escape(self.severity),
            code,
            json_escape(&self.message),
            json_escape(&self.path),
            self.line,
            self.column
        )
    }
}

fn parse_location(line: &str) -> Option<(String, usize, usize)> {
    let (message, location) = line.rsplit_once(" at ")?;
    let (line, column) = location.split_once(':')?;
    Some((
        message.to_string(),
        line.parse().ok()?,
        column.parse().ok()?,
    ))
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch <= '\u{1f}' => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rendered_source_location() {
        let diagnostic = MachineDiagnostic::from_rendered(
            "unknown identifier 'item' at 7:13\n --> /work/main.azk\nitem();\n            ^",
            Path::new("fallback.azk"),
        );
        assert_eq!(diagnostic.message, "unknown identifier 'item'");
        assert_eq!(diagnostic.path, "/work/main.azk");
        assert_eq!((diagnostic.line, diagnostic.column), (7, 13));
    }

    #[test]
    fn emits_valid_escaped_json_contract() {
        let diagnostic = MachineDiagnostic {
            severity: "error",
            code: None,
            message: "bad \"value\"\nnext".to_string(),
            path: "C:\\src\\main.azk".to_string(),
            line: 2,
            column: 4,
        };
        assert_eq!(
            MachineDiagnostic::collection_json("error", &[diagnostic]),
            "{\"schema\":\"aziky-diagnostics-v1\",\"status\":\"error\",\"diagnostics\":[{\"severity\":\"error\",\"message\":\"bad \\\"value\\\"\\nnext\",\"path\":\"C:\\\\src\\\\main.azk\",\"line\":2,\"column\":4}]}"
        );
    }
}
