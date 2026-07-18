/// Deterministic baseline formatter.
///
/// This first contract normalizes newlines, removes trailing whitespace,
/// applies four-space brace indentation, preserves blank lines and comments,
/// and guarantees one final newline. It deliberately does not reflow tokens.
pub fn format_source(source: &str) -> String {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::new();
    let mut depth = 0usize;
    let mut scanner = BraceScanner::default();

    for raw_line in normalized.lines() {
        let content = raw_line.trim();
        if content.is_empty() {
            output.push('\n');
            continue;
        }
        let braces = scanner.scan(content);
        let line_depth = depth.saturating_sub(braces.leading_closes);
        output.push_str(&" ".repeat(line_depth * 4));
        output.push_str(content);
        output.push('\n');
        depth = depth
            .saturating_add(braces.opens)
            .saturating_sub(braces.closes);
    }

    while output.ends_with("\n\n\n") {
        output.pop();
    }
    if output.is_empty() || !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[derive(Default)]
struct BraceScanner {
    block_comment_depth: usize,
}

#[derive(Default)]
struct BraceCount {
    opens: usize,
    closes: usize,
    leading_closes: usize,
}

impl BraceScanner {
    fn scan(&mut self, line: &str) -> BraceCount {
        let bytes = line.as_bytes();
        let mut index = 0;
        let mut result = BraceCount::default();
        let mut quote = None;
        let mut escaped = false;
        let mut seen_non_close = false;
        while index < bytes.len() {
            if self.block_comment_depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    self.block_comment_depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    self.block_comment_depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }
            if let Some(delimiter) = quote {
                if escaped {
                    escaped = false;
                } else if bytes[index] == b'\\' {
                    escaped = true;
                } else if bytes[index] == delimiter {
                    quote = None;
                }
                index += 1;
                continue;
            }
            if bytes[index..].starts_with(b"//") {
                break;
            }
            if bytes[index..].starts_with(b"/*") {
                self.block_comment_depth = 1;
                index += 2;
                continue;
            }
            match bytes[index] {
                b'"' | b'\'' => {
                    quote = Some(bytes[index]);
                    seen_non_close = true;
                }
                b'{' => {
                    result.opens += 1;
                    seen_non_close = true;
                }
                b'}' => {
                    result.closes += 1;
                    if !seen_non_close {
                        result.leading_closes += 1;
                    }
                }
                byte if byte.is_ascii_whitespace() => {}
                _ => seen_non_close = true,
            }
            index += 1;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatter_indents_braces_and_ignores_literals_and_comments() {
        let source = "fn main() {  \r\nlet text: string = \"}\";\r\nif true { // }\r\nprint(text);\r\n}\r\n}\r\n";
        let expected = "fn main() {\n    let text: string = \"}\";\n    if true { // }\n        print(text);\n    }\n}\n";
        assert_eq!(format_source(source), expected);
        assert_eq!(format_source(expected), expected);
    }

    #[test]
    fn formatter_ignores_nested_block_comment_braces() {
        let source = "fn main() {\n/* outer { /* nested } */ still } */\nexit(0u64);\n}\n";
        let expected =
            "fn main() {\n    /* outer { /* nested } */ still } */\n    exit(0u64);\n}\n";
        assert_eq!(format_source(source), expected);
    }
}
