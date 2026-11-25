//! Evaluation of `__dirname`-rooted path expressions.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use oxc_ast::ast::{BinaryOperator, CallExpression, Expression};
use oxc_semantic::{Semantic, SymbolId};
use thinpack_resolve::normalize;

use crate::require;

/// Evaluate a path expression rooted at `__dirname` (`path.join`, `path.resolve`,
/// `join`, `resolve`, or `__dirname + "..."`). Only literal segments and bindings
/// assigned once to one of them are accepted, so arbitrary dynamic paths are left
/// alone.
///
/// A relative result is anchored at `base`, the directory of the module that reads
/// the asset.
pub(super) fn evaluate<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
) -> Option<PathBuf> {
    let value = path_value(expression, base, semantic, &mut BTreeSet::new())?;
    Some(normalize(&if value.path.is_absolute() {
        value.path
    } else {
        base.join(value.path)
    }))
}

/// Evaluate a path expression that is rooted in the file system rather than in
/// the process working directory. Unlike [`evaluate`], a relative
/// `path.resolve(...)` is rejected: Node would resolve it against `cwd`, which
/// packing does not know.
pub(super) fn evaluate_rooted<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
) -> Option<PathBuf> {
    let value = path_value(expression, base, semantic, &mut BTreeSet::new())?;
    (value.rooted && value.path.has_root()).then(|| normalize(&value.path))
}

/// The directory a computed module path selects from, when its literal arguments
/// are followed by one computed final argument. The optional extension is the
/// literal suffix that computed argument appends (`.js` in
/// `` `${name}.js` ``), so packing can keep only the matching candidates.
///
/// The module's own directory and its ancestors are rejected: a computed load
/// there would sweep every sibling instead of a shipped candidate directory.
pub(super) fn module_directory<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
) -> Option<(PathBuf, Option<&'a str>)> {
    let expression = expression.without_parentheses();
    // ``require(`./commands/${command}.js`)`` names a directory next to the module
    // and one literal extension without a `path.join`/`path.resolve` call.
    if let Expression::TemplateLiteral(template) = expression {
        if template.expressions.len() != 1 {
            return None;
        }
        let prefix = template
            .quasis
            .first()?
            .value
            .cooked
            .and_then(oxc_str::JSStr::as_str)?;
        if !prefix.starts_with("./") || !prefix.ends_with('/') {
            return None;
        }
        let directory = normalize(&base.join(prefix));
        if directory == base || !directory.starts_with(base) {
            return None;
        }
        return Some((directory, Some(extension(expression)?)));
    }
    let Expression::CallExpression(call) = expression else {
        return None;
    };
    let callee = match call.callee.without_parentheses() {
        Expression::Identifier(identifier) => identifier.name.as_str(),
        Expression::StaticMemberExpression(member) => member.property.name.as_str(),
        _ => return None,
    };
    if callee != "join" && callee != "resolve" {
        return None;
    }
    let mut arguments = Vec::new();
    let mut dynamic = None;
    for (index, argument) in call.arguments.iter().enumerate() {
        let expression = argument.as_expression()?;
        if let Some(value) = path_value(expression, base, semantic, &mut BTreeSet::new()) {
            arguments.push(value);
            continue;
        }
        // A static argument after the computed one would make the directory
        // depend on the computed value, so only a final computed segment is stable.
        if index + 1 != call.arguments.len() {
            return None;
        }
        dynamic = Some(expression);
        break;
    }
    let dynamic = dynamic?;
    if !arguments
        .first()
        .is_some_and(|first| first.path == base || first.path.starts_with(base))
    {
        return None;
    }
    let path = if callee == "join" {
        join(&arguments)
    } else {
        resolve(&arguments, base)
    };
    if path.path == base || base.starts_with(&path.path) {
        return None;
    }
    Some((path.path, extension(dynamic)))
}

/// The literal extension a computed final path segment appends.
fn extension<'a>(expression: &'a Expression<'a>) -> Option<&'a str> {
    let suffix = match expression.without_parentheses() {
        Expression::TemplateLiteral(template) if !template.expressions.is_empty() => template
            .quasis
            .last()?
            .value
            .cooked
            .and_then(oxc_str::JSStr::as_str)?,
        Expression::BinaryExpression(binary)
            if binary.operator == BinaryOperator::Addition
                && let Expression::StringLiteral(literal) = binary.right.without_parentheses() =>
        {
            literal.value.as_str()?
        }
        _ => return None,
    };
    let extension = suffix.strip_prefix('.')?;
    (!extension.is_empty()
        && extension.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        }))
    .then_some(extension)
}

/// The path an expression evaluates to, before a relative result is anchored,
/// and whether its first segment is rooted away from the working directory.
struct Value {
    path: PathBuf,
    rooted: bool,
}

/// The path an expression evaluates to, before a relative result is anchored.
fn path_value<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
    resolving: &mut BTreeSet<SymbolId>,
) -> Option<Value> {
    match expression.without_parentheses() {
        Expression::Identifier(identifier) if identifier.name == "__dirname" => Some(Value {
            path: base.to_path_buf(),
            rooted: true,
        }),
        // `const dir = __dirname + '/plugins'` is as predictable as the inline
        // expression, so follow a binding that is never reassigned.
        Expression::Identifier(identifier) => {
            let symbol = require::referenced_symbol(identifier, semantic.scoping())?;
            if !resolving.insert(symbol) {
                return None;
            }
            let value = require::initializer(symbol, semantic)
                .and_then(|init| path_value(init, base, semantic, resolving));
            resolving.remove(&symbol);
            value
        }
        Expression::StringLiteral(literal) => literal.value.as_str().map(|value| {
            let path = PathBuf::from(value);
            let rooted = path.has_root();
            Value { path, rooted }
        }),
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            // String concatenation, so an absolute right operand never resets.
            let left = path_value(&binary.left, base, semantic, resolving)?;
            let right = path_value(&binary.right, base, semantic, resolving)?;
            let rooted = left.rooted || (left.path.as_os_str().is_empty() && right.rooted);
            let mut value = left.path.into_os_string();
            value.push(right.path);
            Some(Value {
                path: PathBuf::from(value),
                rooted,
            })
        }
        Expression::CallExpression(call) => call_value(call, base, semantic, resolving),
        _ => None,
    }
}

fn call_value<'a>(
    call: &'a CallExpression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
    resolving: &mut BTreeSet<SymbolId>,
) -> Option<Value> {
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
        .map(|argument| path_value(argument.as_expression()?, base, semantic, resolving))
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
