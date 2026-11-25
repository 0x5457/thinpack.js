//! Evaluation of `__dirname`-rooted path expressions.

use std::path::{Component, Path, PathBuf};

use oxc_ast::ast::{BinaryOperator, CallExpression, Expression};
use thinpack_resolve::normalize;

/// Evaluate a path expression rooted at `__dirname` (`path.join`, `path.resolve`,
/// `join`, `resolve`, or `__dirname + "..."`). Only literal segments are accepted,
/// so arbitrary dynamic paths are left alone.
///
/// A relative result is anchored at `base`, the directory of the module that reads
/// the asset.
pub(super) fn evaluate(expression: &Expression<'_>, base: &Path) -> Option<PathBuf> {
    let value = path_value(expression, base)?;
    Some(normalize(&if value.path.is_absolute() {
        value.path
    } else {
        base.join(value.path)
    }))
}

/// Evaluate a path expression that does not depend on the process working
/// directory. Unlike [`evaluate`], a relative `path.resolve(...)` is rejected:
/// Node would resolve it against `cwd`, which packing does not know.
pub(super) fn evaluate_rooted(expression: &Expression<'_>, base: &Path) -> Option<PathBuf> {
    let value = path_value(expression, base)?;
    (value.rooted && value.path.is_absolute()).then(|| normalize(&value.path))
}

/// The path an expression evaluates to, before a relative result is anchored, and
/// whether it starts from a location other than the working directory.
struct Value {
    path: PathBuf,
    rooted: bool,
}

/// The path an expression evaluates to, before a relative result is anchored.
fn path_value(expression: &Expression<'_>, base: &Path) -> Option<Value> {
    match expression.without_parentheses() {
        Expression::Identifier(identifier) if identifier.name == "__dirname" => Some(Value {
            path: base.to_path_buf(),
            rooted: true,
        }),
        Expression::StringLiteral(literal) => literal.value.as_str().map(|value| {
            let path = PathBuf::from(value);
            let rooted = path.is_absolute();
            Value { path, rooted }
        }),
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            // String concatenation, so an absolute right operand never resets.
            let left = path_value(&binary.left, base)?;
            let right = path_value(&binary.right, base)?;
            let rooted = left.rooted || (left.path.as_os_str().is_empty() && right.rooted);
            let mut value = left.path.into_os_string();
            value.push(right.path);
            Some(Value {
                path: PathBuf::from(value),
                rooted,
            })
        }
        Expression::CallExpression(call) => call_value(call, base),
        _ => None,
    }
}

fn call_value(call: &CallExpression<'_>, base: &Path) -> Option<Value> {
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
fn join(arguments: &[Value]) -> Value {
    let mut joined = PathBuf::new();
    let mut rooted = false;
    let mut started = false;
    for argument in arguments
        .iter()
        .filter(|argument| !argument.path.as_os_str().is_empty())
    {
        if !started {
            rooted = argument.rooted;
        }
        for component in argument.path.components() {
            if started && matches!(component, Component::Prefix(_) | Component::RootDir) {
                continue;
            }
            joined.push(component.as_os_str());
        }
        started = true;
    }
    Value {
        path: normalize(&joined),
        rooted,
    }
}

/// `path.resolve`: a rooted argument resets the result, so
/// `resolve(__dirname, "/asset")` leaves the module directory. `base` stands in for
/// the process working directory; the result is rooted only when an argument
/// overrides it.
fn resolve(arguments: &[Value], base: &Path) -> Value {
    let mut resolved = base.to_path_buf();
    for argument in arguments {
        if argument.path.has_root() {
            resolved.clone_from(&argument.path);
        } else {
            resolved.push(&argument.path);
        }
    }
    Value {
        path: normalize(&resolved),
        rooted: arguments.iter().any(|argument| argument.rooted),
    }
}
