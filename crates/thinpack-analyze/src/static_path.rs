//! Evaluation of `__dirname`-rooted path expressions.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use oxc_ast::ast::{Argument, BinaryOperator, CallExpression, Expression};
use oxc_semantic::Semantic;
use thinpack_resolve::normalize;

use crate::require;

/// Evaluate a path expression rooted at `__dirname` (`path.join`, `path.resolve`,
/// `join`, `resolve`, or `__dirname + "..."`). Only literal segments are accepted,
/// so arbitrary dynamic paths are left alone.
///
/// A relative result is anchored at `base`, the directory of the module that reads
/// the asset.
pub(super) fn evaluate(expression: &Expression<'_>, base: &Path) -> Option<PathBuf> {
    let (path, complete, _) = prefix_value(expression, base)?;
    complete.then(|| anchor(path, base))
}

/// Directories a module load can choose a file from when its path names a literal
/// `__dirname`-rooted directory and continues with a computed segment.
///
/// The binding the load names is followed through its assignments, which is how
/// `require(formatterPath)` reaches `path.resolve(__dirname, "formatters", name)`.
/// A fully static expression is left to ordinary resolution, and the module's own
/// directory and its ancestors are excluded so a computed load does not sweep
/// every sibling by default.
pub(super) fn computed_directories<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
) -> BTreeSet<PathBuf> {
    let mut directories = BTreeSet::new();
    collect_directories(
        expression.without_parentheses(),
        base,
        semantic,
        &mut directories,
    );
    directories.retain(|directory| directory != base && !base.starts_with(directory));
    directories
}

fn collect_directories<'a>(
    expression: &'a Expression<'a>,
    base: &Path,
    semantic: &Semantic<'a>,
    directories: &mut BTreeSet<PathBuf>,
) {
    match expression {
        // `import(pathToFileURL(formatterPath))` names a path in its first argument.
        Expression::CallExpression(call) if is_path_to_file_url(call) => {
            if let Some(argument) = call.arguments.first().and_then(Argument::as_expression) {
                collect_directories(argument.without_parentheses(), base, semantic, directories);
            }
        }
        Expression::Identifier(identifier) => {
            let Some(symbol) = require::referenced_symbol(identifier, semantic.scoping()) else {
                return;
            };
            for value in require::assigned_values(symbol, semantic) {
                if let Some((path, false, true)) = prefix_value(value, base) {
                    directories.insert(anchor(path, base));
                }
            }
        }
        _ => {
            if let Some((path, false, true)) = prefix_value(expression, base) {
                directories.insert(anchor(path, base));
            }
        }
    }
}

fn is_path_to_file_url(call: &CallExpression<'_>) -> bool {
    matches!(
        call.callee.without_parentheses(),
        Expression::Identifier(identifier) if identifier.name == "pathToFileURL"
    ) || matches!(
        call.callee.without_parentheses(),
        Expression::StaticMemberExpression(member) if member.property.name == "pathToFileURL"
    )
}

fn anchor(path: PathBuf, base: &Path) -> PathBuf {
    normalize(&if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

/// The path a `path.join`/`path.resolve` expression reaches before its first
/// computed argument, whether every argument was literal, and whether the first
/// argument was `__dirname` rather than a relative path.
fn prefix_value(expression: &Expression<'_>, base: &Path) -> Option<(PathBuf, bool, bool)> {
    match expression.without_parentheses() {
        Expression::Identifier(identifier) if identifier.name == "__dirname" => {
            Some((base.to_path_buf(), true, true))
        }
        Expression::StringLiteral(literal) => literal
            .value
            .as_str()
            .map(|value| (PathBuf::from(value), true, false)),
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            let (left, left_complete, rooted) = prefix_value(&binary.left, base)?;
            if !left_complete {
                return Some((left, false, rooted));
            }
            let Some((right, right_complete, _)) = prefix_value(&binary.right, base) else {
                return Some((left, false, rooted));
            };
            let mut value = left.into_os_string();
            value.push(right);
            Some((PathBuf::from(value), right_complete, rooted))
        }
        Expression::CallExpression(call) => prefix_call(call, base),
        _ => None,
    }
}

fn prefix_call(call: &CallExpression<'_>, base: &Path) -> Option<(PathBuf, bool, bool)> {
    let callee = match call.callee.without_parentheses() {
        Expression::Identifier(identifier) => identifier.name.as_str(),
        Expression::StaticMemberExpression(member) => member.property.name.as_str(),
        _ => return None,
    };
    if callee != "join" && callee != "resolve" {
        return None;
    }
    let incomplete = |arguments: &[PathBuf], rooted: bool| {
        (!arguments.is_empty()).then(|| {
            let path = if callee == "join" {
                join(arguments)
            } else {
                resolve(arguments, base)
            };
            (path, false, rooted)
        })
    };
    let mut arguments = Vec::new();
    let mut rooted = false;
    for argument in &call.arguments {
        let Some((value, complete, value_rooted)) = argument
            .as_expression()
            .and_then(|argument| prefix_value(argument, base))
        else {
            // The first computed argument ends the static prefix. Without one, the
            // call was not rooted at `__dirname` either.
            return incomplete(&arguments, rooted);
        };
        if arguments.is_empty() {
            rooted = value_rooted;
        }
        if !complete {
            return incomplete(&arguments, rooted);
        }
        arguments.push(value);
    }
    let path = if callee == "join" {
        join(&arguments)
    } else {
        resolve(&arguments, base)
    };
    Some((path, true, rooted))
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
