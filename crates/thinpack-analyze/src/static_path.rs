//! Evaluation of `__dirname`-rooted path expressions.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use oxc_ast::ast::{BinaryExpression, BinaryOperator, CallExpression, Expression};
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
    evaluate_with_bindings(expression, base, semantic, &BTreeMap::new())
}

/// [`evaluate`] with parameter bindings supplied by a call of the same-file
/// function the read belongs to. A bound expression replaces the declaration
/// lookup for its symbol.
pub(super) fn evaluate_with_bindings<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
    bindings: &BTreeMap<SymbolId, &'a Expression<'a>>,
) -> Option<PathBuf> {
    let value = path_value(expression, base, semantic, bindings, &mut BTreeSet::new())?;
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
    let value = path_value(
        expression,
        base,
        semantic,
        &BTreeMap::new(),
        &mut BTreeSet::new(),
    )?;
    (value.rooted && value.path.has_root()).then(|| normalize(&value.path))
}

/// The directory a computed module path selects from, when a literal prefix
/// names it and one final computed segment follows. The optional extension is
/// the literal suffix that segment appends (`.js` in
/// `` `${name}.js` ``), so packing can keep only the matching candidates.
///
/// A concatenated prefix has to end with a separator, because `'./lib' + name`
/// can name the sibling `lib-extra` as well as a child of `./lib`; a literal
/// suffix after the computed segment has to be an extension, because anything
/// else names a path below the directory rather than an entry in it.
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
    if let Expression::BinaryExpression(binary) = expression {
        return binary_directory(binary, base, semantic, expression);
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
        if let Some(value) = path_value(
            expression,
            base,
            semantic,
            &BTreeMap::new(),
            &mut BTreeSet::new(),
        ) {
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
    let filter = match candidates(dynamic) {
        Candidates::Modules => None,
        Candidates::Extension(extension) => Some(extension),
        Candidates::BelowDirectory => return None,
    };
    Some((path.path, filter))
}

/// ``require('./lib/grant/' + name)`` names a directory next to the module and
/// loads the name the final computed segment selects. A concatenated literal
/// suffix, as in ``'./plugins/' + name + '.js'``, keeps only that extension.
fn binary_directory<'a>(
    binary: &'a BinaryExpression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
    expression: &'a Expression<'a>,
) -> Option<(PathBuf, Option<&'a str>)> {
    if binary.operator != BinaryOperator::Addition {
        return None;
    }
    if path_value(
        &binary.right,
        base,
        semantic,
        &BTreeMap::new(),
        &mut BTreeSet::new(),
    )
    .is_some()
    {
        // The appended literal is a suffix on the computed segment, so only an
        // extension keeps it inside the directory; `'/index.js'` names a path
        // below it, and a non-extension suffix cannot be a candidate filter.
        let (directory, _) = module_directory(&binary.left, base, semantic)?;
        return Some((directory, Some(extension(expression)?)));
    }
    let value = path_value(
        &binary.left,
        base,
        semantic,
        &BTreeMap::new(),
        &mut BTreeSet::new(),
    )?;
    if !value.rooted && !value.path.starts_with(".") {
        return None;
    }
    // `'./plugins' + name` can name the sibling `plugins-extra`, not just an
    // entry of `./plugins`, so only a prefix that ends with a separator proves
    // the directory.
    if !value.path.to_string_lossy().ends_with('/') {
        return None;
    }
    let directory = normalize(&if value.path.is_absolute() {
        value.path
    } else {
        base.join(value.path)
    });
    if directory == base || !directory.starts_with(base) {
        return None;
    }
    let filter = match candidates(&binary.right) {
        Candidates::Modules => None,
        Candidates::Extension(extension) => Some(extension),
        Candidates::BelowDirectory => return None,
    };
    Some((directory, filter))
}

/// The candidates a computed final path segment's literal suffix leaves.
enum Candidates<'a> {
    /// No literal suffix, so every JavaScript module in the directory matches.
    Modules,
    /// The suffix is this extension, so only files ending with it match.
    Extension(&'a str),
    /// The suffix is not an extension, so it names a path below the directory,
    /// which no filter on the directory itself can bound.
    BelowDirectory,
}

/// The candidates a computed final path segment selects.
fn candidates<'a>(expression: &'a Expression<'a>) -> Candidates<'a> {
    match suffix(expression) {
        None => Candidates::Modules,
        Some(_) => extension(expression).map_or(Candidates::BelowDirectory, Candidates::Extension),
    }
}

/// The literal suffix a computed final path segment appends, if it appends one.
fn suffix<'a>(expression: &'a Expression<'a>) -> Option<&'a str> {
    match expression.without_parentheses() {
        Expression::TemplateLiteral(template) if !template.expressions.is_empty() => template
            .quasis
            .last()?
            .value
            .cooked
            .and_then(oxc_str::JSStr::as_str),
        Expression::BinaryExpression(binary)
            if binary.operator == BinaryOperator::Addition
                && let Expression::StringLiteral(literal) = binary.right.without_parentheses() =>
        {
            literal.value.as_str()
        }
        _ => None,
    }
}

/// The literal extension a computed final path segment appends.
fn extension<'a>(expression: &'a Expression<'a>) -> Option<&'a str> {
    let extension = suffix(expression)?.strip_prefix('.')?;
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
    bindings: &BTreeMap<SymbolId, &'a Expression<'a>>,
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
            if let Some(bound) = bindings.get(&symbol) {
                return path_value(bound, base, semantic, bindings, resolving);
            }
            if !resolving.insert(symbol) {
                return None;
            }
            let value = require::initializer(symbol, semantic)
                .and_then(|init| path_value(init, base, semantic, bindings, resolving));
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
            let left = path_value(&binary.left, base, semantic, bindings, resolving)?;
            let right = path_value(&binary.right, base, semantic, bindings, resolving)?;
            let rooted = left.rooted || (left.path.as_os_str().is_empty() && right.rooted);
            let mut value = left.path.into_os_string();
            value.push(right.path);
            Some(Value {
                path: PathBuf::from(value),
                rooted,
            })
        }
        Expression::CallExpression(call) => call_value(call, base, semantic, bindings, resolving),
        _ => None,
    }
}

fn call_value<'a>(
    call: &'a CallExpression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
    bindings: &BTreeMap<SymbolId, &'a Expression<'a>>,
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
        .map(|argument| {
            path_value(
                argument.as_expression()?,
                base,
                semantic,
                bindings,
                resolving,
            )
        })
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
