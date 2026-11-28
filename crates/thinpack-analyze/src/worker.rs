//! Static `worker_threads.Worker` entries named by a module-relative URL.
//!
//! A worker starts a module of its own, so the worker file and everything it
//! imports have to join the payload. Only a named import of Node's `Worker` and
//! a named import of Node's `URL` prove the call: a writable global, a `require`
//! destructure, or a namespace member can be replaced before the call.

use std::path::{Path, PathBuf};

use oxc_ast::ast::{Argument, Expression, NewExpression, ObjectPropertyKind};
use oxc_semantic::Semantic;
use thinpack_resolve::normalize;

use crate::require;

/// The path a `new Worker(new URL(<relative>, import.meta.url))` call loads.
///
/// `Worker` has to be the immutable named import from `worker_threads`, `URL`
/// the immutable named import from `url`, and the options have to leave the
/// first argument a filename. A worker path the program computes stays a runtime
/// value and still needs `--include`.
pub(super) fn entry<'a>(
    call: &'a NewExpression<'a>,
    semantic: &Semantic<'a>,
    module: &Path,
) -> Option<PathBuf> {
    if !require::named_import(
        &call.callee,
        semantic,
        "Worker",
        &["worker_threads", "node:worker_threads"],
    ) || !options_are_filename(&call.arguments)
    {
        return None;
    }
    let Expression::NewExpression(url) = call
        .arguments
        .first()?
        .as_expression()?
        .without_parentheses()
    else {
        return None;
    };
    if !require::named_import(&url.callee, semantic, "URL", &["url", "node:url"])
        || !url
            .arguments
            .get(1)
            .and_then(Argument::as_expression)
            .is_some_and(require::is_import_meta_url)
    {
        return None;
    }
    let target = match url
        .arguments
        .first()?
        .as_expression()?
        .without_parentheses()
    {
        Expression::StringLiteral(literal) => literal.value.as_str()?,
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            template.quasis.first()?.value.cooked?.as_str()?
        }
        _ => return None,
    };
    relative_path(target, module)
}

/// The path a relative URL path names from the module at `module`.
///
/// An absolute URL, an encoded segment, a query, a fragment, or whitespace the
/// parser strips would need URL evaluation the analyzer does not do; a path join
/// reproduces the remaining literals exactly.
fn relative_path(literal: &str, module: &Path) -> Option<PathBuf> {
    if literal.starts_with('/')
        || literal.ends_with('/')
        || literal.trim() != literal
        || literal.chars().any(|character| {
            matches!(character, '?' | '#' | '\\' | ':' | '%') || character.is_ascii_control()
        })
    {
        return None;
    }
    let relative = Path::new(literal);
    // A URL that names a directory, or nothing, is not a module. A final `.`
    // segment is a directory URL even though `Path` drops it as a component.
    relative.file_name()?;
    if matches!(literal.rsplit('/').next(), Some("." | "..")) {
        return None;
    }
    Some(normalize(
        &module
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .join(relative),
    ))
}

/// Whether the `Worker` options leave the first argument a filename.
///
/// `eval: true` compiles the first argument as source. The constructor defaults
/// `eval` to false, so a plain object literal proves the filename when every
/// property is unambiguous and `eval`, if present, is a boolean `false`. A
/// spread, a computed key, an accessor, `__proto__`, or any other `eval` leaves
/// the effective value unproved.
fn options_are_filename(arguments: &[Argument<'_>]) -> bool {
    let Some(options) = arguments.get(1).and_then(Argument::as_expression) else {
        return true;
    };
    let Some(object) = require::plain_object(options) else {
        return false;
    };
    object.properties.iter().all(|entry| {
        // `plain_object` rejects every other property kind.
        let ObjectPropertyKind::ObjectProperty(property) = entry else {
            return true;
        };
        require::property_key(&property.key) != Some("eval")
            || matches!(
                property.value.without_parentheses(),
                Expression::BooleanLiteral(literal) if !literal.value
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_url_paths_reject_literals_the_url_parser_rewrites() {
        let module = Path::new("/app/main.mjs");
        assert_eq!(
            relative_path("./sub/../worker.mjs", module),
            Some(PathBuf::from("/app/worker.mjs"))
        );
        for literal in [
            "/app/worker.mjs",
            "//host/worker.mjs",
            "./worker.mjs?x",
            "./worker.mjs#f",
            "./w%20r.mjs",
            "src\\worker.mjs",
            "c:/worker.mjs",
            ".",
            "..",
            "./worker.mjs/.",
            "./worker.mjs/..",
            "./dir/",
            "",
            " ./worker.mjs",
            "./worker.mjs ",
            "./wor\tker.mjs",
            "./wor\u{0}ker.mjs",
        ] {
            assert_eq!(relative_path(literal, module), None, "{literal:?}");
        }
    }
}
