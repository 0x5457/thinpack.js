//! Evaluation of `__dirname`-rooted path expressions.

use std::path::{Component, Path, PathBuf};

use oxc_ast::ast::{Argument, BinaryOperator, CallExpression, Expression};
use thinpack_resolve::normalize;

/// Evaluate a path expression rooted at `__dirname` (`path.join`, `path.resolve`,
/// `join`, `resolve`, or `__dirname + "..."`). Only literal segments are accepted,
/// so arbitrary dynamic paths are left alone.
///
/// A relative result is anchored at `base`, the directory of the module that reads
/// the asset.
pub(super) fn evaluate(expression: &Expression<'_>, base: &Path) -> Option<PathBuf> {
    let path = path_value(expression, base)?;
    Some(normalize(&if path.is_absolute() {
        path
    } else {
        base.join(path)
    }))
}

/// The path an expression evaluates to, before a relative result is anchored.
fn path_value(expression: &Expression<'_>, base: &Path) -> Option<PathBuf> {
    match expression.without_parentheses() {
        Expression::Identifier(identifier) if identifier.name == "__dirname" => {
            Some(base.to_path_buf())
        }
        Expression::StringLiteral(literal) => literal.value.as_str().map(PathBuf::from),
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            // String concatenation, so an absolute right operand never resets.
            let mut value = path_value(&binary.left, base)?.into_os_string();
            value.push(path_value(&binary.right, base)?);
            Some(PathBuf::from(value))
        }
        Expression::CallExpression(call) => call_value(call, base),
        _ => None,
    }
}

fn call_value(call: &CallExpression<'_>, base: &Path) -> Option<PathBuf> {
    let callee = match call.callee.without_parentheses() {
        Expression::Identifier(identifier) => identifier.name.as_str(),
        Expression::StaticMemberExpression(member) => member.property.name.as_str(),
        _ => return None,
    };
    if callee != "join" && callee != "resolve" {
        return None;
    }
    let arguments = call
        .arguments
        .iter()
        .map(|argument| path_value(argument.as_expression()?, base))
        .collect::<Option<Vec<_>>>()?;
    Some(if callee == "join" {
        join(&arguments)
    } else {
        resolve(&arguments, base)
    })
}

/// `path.join`: concatenate the arguments, then normalize. A leading separator on a
/// later argument does not reset the path, so `join(__dirname, "/asset")` stays
/// inside `__dirname`.
fn join(arguments: &[PathBuf]) -> PathBuf {
    let mut joined = PathBuf::new();
    let mut started = false;
    for argument in arguments
        .iter()
        .filter(|argument| !argument.as_os_str().is_empty())
    {
        for component in argument.components() {
            if started && matches!(component, Component::Prefix(_) | Component::RootDir) {
                continue;
            }
            joined.push(component.as_os_str());
        }
        started = true;
    }
    normalize(&joined)
}

/// `path.resolve`: a rooted argument resets the result, so
/// `resolve(__dirname, "/asset")` leaves the module directory. `base` stands in for
/// the process working directory.
fn resolve(arguments: &[PathBuf], base: &Path) -> PathBuf {
    let mut resolved = base.to_path_buf();
    for argument in arguments {
        if argument.has_root() {
            resolved.clone_from(argument);
        } else {
            resolved.push(argument);
        }
    }
    normalize(&resolved)
}
