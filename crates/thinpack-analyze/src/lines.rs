use std::path::Path;

use oxc_allocator::Allocator;
use oxc_parser::{Kind, Parser, config::TokensParserConfig};
use oxc_span::SourceType;

pub const LINE_LIMIT: usize = 1024;

#[must_use]
pub fn bound_lines(source: &str, path: &Path, limit: usize) -> Option<String> {
    if !source.is_ascii() || source.lines().all(|line| line.len() <= limit) {
        return None;
    }

    let source_type = SourceType::from_path(path).ok()?;
    let allocator = Allocator::default();
    let parsed = [true, false]
        .into_iter()
        .map(|module| {
            Parser::new(&allocator, source, source_type.with_module(module))
                .with_config(TokensParserConfig)
                .parse()
        })
        .find(|parsed| !parsed.fatal_error && parsed.diagnostics.is_empty())?;

    let tokens: Vec<(usize, usize)> = parsed
        .tokens
        .iter()
        .filter(|token| !matches!(token.kind(), Kind::Eof))
        .map(|token| (token.start() as usize, token.end() as usize))
        .collect();

    let mut out = String::with_capacity(source.len() + source.len() / limit);
    let mut cursor = 0;
    let mut line = 0;
    let mut changed = false;

    for pair in tokens.windows(2) {
        let (previous_start, previous_end) = pair[0];
        let (next_start, next_end) = pair[1];
        push(&mut out, &mut line, &source[cursor..previous_end]);

        let gap = &source[previous_end..next_start];
        let previous = &source[previous_start..previous_end];
        let next = &source[next_start..next_end];

        if !gap.contains('\n')
            && line + gap.len() + next.len() > limit
            && is_safe_boundary(previous, next)
        {
            out.push('\n');
            line = 0;
            changed = true;
        } else {
            push(&mut out, &mut line, gap);
        }
        cursor = next_start;
    }
    push(&mut out, &mut line, &source[cursor..]);

    changed.then_some(out)
}

fn push(out: &mut String, line: &mut usize, text: &str) {
    out.push_str(text);
    match text.rfind('\n') {
        Some(index) => *line = text.len() - index - 1,
        None => *line += text.len(),
    }
}

fn is_safe_boundary(previous: &str, next: &str) -> bool {
    if matches!(
        previous,
        "return" | "throw" | "break" | "continue" | "yield" | "async" | "await"
    ) {
        return false;
    }
    if matches!(next, "++" | "--") {
        return false;
    }
    continues_expression(previous) || starts_continuation(next)
}

fn continues_expression(token: &str) -> bool {
    matches!(
        token,
        ";" | ","
            | "{"
            | "("
            | "["
            | ":"
            | "?"
            | "."
            | "?."
            | "=>"
            | "="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "&&"
            | "||"
            | "??"
            | "&"
            | "|"
            | "^"
            | "!"
            | "~"
            | "<"
            | ">"
            | "<="
            | ">="
            | "=="
            | "==="
            | "!="
            | "!=="
            | "<<"
            | ">>"
            | "+="
            | "-="
            | "*="
            | "/="
    )
}

fn starts_continuation(token: &str) -> bool {
    matches!(
        token,
        "." | "?."
            | "("
            | "["
            | ","
            | ")"
            | "]"
            | "}"
            | ":"
            | "?"
            | "=>"
            | "="
            | "&&"
            | "||"
            | "??"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use oxc_allocator::Allocator;
    use oxc_parser::{Parser, config::TokensParserConfig};
    use oxc_span::SourceType;

    use super::bound_lines;

    fn bound(source: &str, limit: usize) -> String {
        bound_lines(source, Path::new("x.js"), limit).unwrap_or_else(|| source.to_owned())
    }

    fn tokens(source: &str) -> Vec<String> {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::from_path("x.js").unwrap())
            .with_config(TokensParserConfig)
            .parse();
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        parsed
            .tokens
            .iter()
            .map(|token| source[token.start() as usize..token.end() as usize].to_owned())
            .collect()
    }

    #[test]
    fn short_lines_are_left_alone() {
        assert_eq!(bound_lines("const a = 1;\n", Path::new("x.js"), 40), None);
    }

    #[test]
    fn long_lines_are_broken_without_changing_tokens() {
        let source = format!(
            "{};",
            (0..40)
                .map(|index| format!("var value{index} = {index}"))
                .collect::<Vec<_>>()
                .join(";")
        );
        let out = bound(&source, 40);
        assert!(out.len() > source.len());
        assert!(out.lines().any(|line| line.len() <= 40));
        assert_eq!(tokens(&source), tokens(&out));
    }

    #[test]
    fn return_never_gains_a_line_break() {
        let source = format!("function f(){{ return '{}' }}", "a".repeat(80));
        let out = bound(&source, 20);
        assert!(!out.contains("return\n"), "{out}");
    }

    #[test]
    fn postfix_update_stays_on_its_line() {
        let source = format!("var pad = '{}'; counter++;", "a".repeat(60));
        let out = bound(&source, 20);
        assert!(!out.contains("counter\n++"), "{out}");
        assert_eq!(tokens(&source), tokens(&out));
    }

    #[test]
    fn nested_calls_break_inside_arguments() {
        let source = format!(
            "callSomething({});",
            (0..12)
                .map(|index| format!("someArgument{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let out = bound(&source, 40);
        assert!(out.lines().count() > 1, "{out}");
        assert_eq!(tokens(&source), tokens(&out));
    }
}
