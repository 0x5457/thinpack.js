use std::fmt::Write as _;

/// Quotes `value` as a double-quoted JavaScript string literal.
#[must_use]
pub fn js_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for character in value.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                let _ = write!(literal, "\\u{:04x}", other as u32);
            }
            other => literal.push(other),
        }
    }
    literal.push('"');
    literal
}
