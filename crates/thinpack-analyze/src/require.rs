use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use oxc_ast::AstKind;
use oxc_ast::ast::{
    BinaryOperator, Expression, IdentifierReference, ObjectPropertyKind, PropertyKey, PropertyKind,
    Statement,
};
use oxc_semantic::{IsGlobalReference, Scoping, Semantic, SymbolId};
use oxc_span::GetSpan;

pub(super) fn bindings(semantic: &Semantic<'_>) -> BTreeSet<SymbolId> {
    semantic
        .scoping()
        .symbol_ids()
        .filter(|&symbol| {
            initializer(symbol, semantic).is_some_and(|init| {
                is_runtime_require(init, semantic)
                    || is_create_require_call(init, semantic)
                    || is_esbuild_wrapper(init, semantic)
            })
        })
        .collect()
}

pub(super) fn referenced_symbol(
    identifier: &IdentifierReference<'_>,
    scoping: &Scoping,
) -> Option<SymbolId> {
    scoping
        .get_reference(identifier.reference_id.get()?)
        .symbol_id()
}

enum PendingSpecifier<'a, 'e> {
    Expression(&'e Expression<'a>),
    Resolved(SymbolId),
}

pub(super) fn specifiers<'a>(
    argument: &Expression<'a>,
    semantic: &Semantic<'a>,
    mut add: impl FnMut(String),
) -> bool {
    let argument = argument.without_parentheses();
    if let Some(value) = static_string(argument, semantic) {
        add(value);
        return true;
    }

    let mut pending = vec![PendingSpecifier::Expression(argument)];
    let mut bindings = BTreeMap::new();
    let mut complete = true;
    while let Some(next) = pending.pop() {
        let expression = match next {
            PendingSpecifier::Expression(expression) => expression.without_parentheses(),
            PendingSpecifier::Resolved(symbol) => {
                bindings.insert(symbol, true);
                continue;
            }
        };
        match expression {
            Expression::StringLiteral(literal) => add(super::js_string(literal.value)),
            Expression::ConditionalExpression(branch) => {
                pending.push(PendingSpecifier::Expression(&branch.alternate));
                pending.push(PendingSpecifier::Expression(&branch.consequent));
            }
            Expression::Identifier(identifier) => {
                let Some(symbol) = referenced_symbol(identifier, semantic.scoping()) else {
                    complete = false;
                    continue;
                };
                match bindings.entry(symbol) {
                    // A binding already on this path makes the candidate set incomplete.
                    Entry::Occupied(entry) => complete &= *entry.get(),
                    Entry::Vacant(entry) => {
                        let Some(init) = initializer(symbol, semantic) else {
                            complete = false;
                            continue;
                        };
                        entry.insert(false);
                        pending.push(PendingSpecifier::Resolved(symbol));
                        pending.push(PendingSpecifier::Expression(init));
                    }
                }
            }
            _ => complete = false,
        }
    }
    complete
}

/// A string the expression always produces: literals, immutable bindings, template
/// literals, and properties of inline object literals.
///
/// A launcher commonly names another package and its bin through a constant object,
/// as webpack does with `cli.package` and `cli.binName`. Evaluating that constant lets
/// the analyzer resolve the manifest instead of treating the request as dynamic.
fn static_string(expression: &Expression<'_>, semantic: &Semantic<'_>) -> Option<String> {
    static_string_with(expression, semantic, &mut BTreeSet::new())
}

fn static_string_with(
    expression: &Expression<'_>,
    semantic: &Semantic<'_>,
    visiting: &mut BTreeSet<SymbolId>,
) -> Option<String> {
    match expression.without_parentheses() {
        Expression::StringLiteral(literal) => Some(super::js_string(literal.value)),
        Expression::TemplateLiteral(template) => {
            let mut value = String::new();
            for (index, quasi) in template.quasis.iter().enumerate() {
                value.push_str(quasi.value.cooked?.as_str()?);
                if let Some(expression) = template.expressions.get(index) {
                    value.push_str(&static_string_with(expression, semantic, visiting)?);
                }
            }
            Some(value)
        }
        Expression::Identifier(identifier) => {
            let symbol = referenced_symbol(identifier, semantic.scoping())?;
            if !is_constant(symbol, semantic) || !visiting.insert(symbol) {
                return None;
            }
            let value = if let Some(init) = initializer(symbol, semantic) {
                static_string_with(init, semantic, visiting)
            } else {
                parameter_value(symbol, semantic, |argument| {
                    static_string_with(argument, semantic, visiting)
                })
            };
            visiting.remove(&symbol);
            value
        }
        Expression::StaticMemberExpression(member) => {
            let Expression::Identifier(object) = member.object.without_parentheses() else {
                return None;
            };
            let symbol = referenced_symbol(object, semantic.scoping())?;
            if !is_constant(symbol, semantic) || !visiting.insert(symbol) {
                return None;
            }
            let value = if let Some(init) = initializer(symbol, semantic) {
                object_value(init, member.property.name.as_str(), semantic, visiting)
            } else {
                parameter_value(symbol, semantic, |argument| {
                    object_value(argument, member.property.name.as_str(), semantic, visiting)
                })
            };
            visiting.remove(&symbol);
            value
        }
        _ => None,
    }
}

/// The value a parameter receives when every call site agrees.
///
/// Webpack names webpack-cli through a constant object that it passes to its launcher,
/// so the manifest request is only static after looking through those calls.
fn parameter_value<T: PartialEq>(
    symbol: SymbolId,
    semantic: &Semantic<'_>,
    mut evaluate: impl FnMut(&Expression<'_>) -> Option<T>,
) -> Option<T> {
    let declaration = semantic.symbol_declaration(symbol);
    declaration.kind().as_formal_parameter()?;
    if !is_constant(symbol, semantic) {
        return None;
    }
    let nodes = semantic.nodes();
    let (index, parameters) = nodes.ancestors(declaration.id()).find_map(|node| {
        let parameters = node.kind().as_formal_parameters()?;
        let index = parameters
            .items
            .iter()
            .position(|item| item.node_id.get() == declaration.id())?;
        Some((index, node))
    })?;
    let function = nodes.parent_node(parameters.id());
    let function_symbol = match function.kind() {
        AstKind::Function(function) => function.id.as_ref()?.symbol_id.get()?,
        AstKind::ArrowFunctionExpression(_) => {
            let AstKind::VariableDeclarator(declaration) = nodes.parent_node(function.id()).kind()
            else {
                return None;
            };
            declaration.id.get_binding_identifier()?.symbol_id.get()?
        }
        _ => return None,
    };

    let mut value = None;
    for reference in semantic.symbol_references(function_symbol) {
        let node = nodes.get_node(reference.node_id());
        let AstKind::CallExpression(call) = nodes.parent_node(node.id()).kind() else {
            return None;
        };
        if call.callee.without_parentheses().span() != node.kind().span() {
            return None;
        }
        let argument = call
            .arguments
            .get(index)
            .and_then(|argument| argument.as_expression())?;
        let candidate = evaluate(argument)?;
        if value.as_ref().is_some_and(|value| value != &candidate) {
            return None;
        }
        value = Some(candidate);
    }
    value
}

fn object_value(
    object: &Expression<'_>,
    property: &str,
    semantic: &Semantic<'_>,
    visiting: &mut BTreeSet<SymbolId>,
) -> Option<String> {
    let object = match object.without_parentheses() {
        Expression::ObjectExpression(object) => object,
        Expression::Identifier(identifier) => {
            let symbol = referenced_symbol(identifier, semantic.scoping())?;
            if !is_constant(symbol, semantic) || !visiting.insert(symbol) {
                return None;
            }
            let value = initializer(symbol, semantic);
            visiting.remove(&symbol);
            let Expression::ObjectExpression(object) = value?.without_parentheses() else {
                return None;
            };
            object
        }
        _ => return None,
    };
    object.properties.iter().find_map(|entry| {
        let ObjectPropertyKind::ObjectProperty(property_entry) = entry else {
            return None;
        };
        if property_entry.kind != PropertyKind::Init || property_entry.method {
            return None;
        }
        let key = match &property_entry.key {
            PropertyKey::StaticIdentifier(identifier) => identifier.name.as_str(),
            PropertyKey::StringLiteral(literal) => literal.value.as_str()?,
            _ => return None,
        };
        (key == property)
            .then(|| static_string_with(&property_entry.value, semantic, visiting))
            .flatten()
    })
}

fn initializer<'a>(symbol: SymbolId, semantic: &Semantic<'a>) -> Option<&'a Expression<'a>> {
    let AstKind::VariableDeclarator(declaration) = semantic.symbol_declaration(symbol).kind()
    else {
        return None;
    };
    let init = declaration.init.as_ref()?;
    let scoping = semantic.scoping();
    (declaration.id.get_binding_identifier().is_some()
        && scoping.symbol_redeclarations(symbol).is_empty()
        && !scoping.symbol_is_mutated(symbol))
    .then(|| init.without_parentheses())
}

/// Whether every reference only reads the binding. A property assignment mutates
/// the object without writing the binding itself, so the member target flag matters.
fn is_constant(symbol: SymbolId, semantic: &Semantic<'_>) -> bool {
    !semantic.scoping().symbol_is_mutated(symbol)
        && semantic
            .symbol_references(symbol)
            .all(|reference| !reference.flags().is_member_write_target())
}

fn is_runtime_require(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    let Some(identifier) = expression.without_parentheses().get_identifier_reference() else {
        return false;
    };
    identifier.name == "require"
        && (identifier.is_global_reference(semantic.scoping())
            || referenced_symbol(identifier, semantic.scoping())
                .and_then(|symbol| initializer(symbol, semantic))
                .is_some_and(|init| is_create_require_call(init, semantic)))
}

fn is_create_require_call(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    let Expression::CallExpression(call) = expression else {
        return false;
    };
    let Some(callee) = call.callee.without_parentheses().get_identifier_reference() else {
        return false;
    };
    let Some(symbol) = referenced_symbol(callee, semantic.scoping()) else {
        return false;
    };
    let declaration = semantic.symbol_declaration(symbol);
    matches!(declaration.kind(), AstKind::ImportSpecifier(specifier)
        if specifier.imported.name() == "createRequire")
        && matches!(semantic.nodes().parent_kind(declaration.id()),
            AstKind::ImportDeclaration(import) if matches!(import.source.value.as_str(), Some("node:module" | "module")))
}

fn is_esbuild_wrapper(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    let Expression::CallExpression(call) = expression else {
        return false;
    };
    let result = match call.callee.without_parentheses() {
        Expression::ArrowFunctionExpression(factory) if !factory.r#async => {
            factory.get_expression()
        }
        Expression::FunctionExpression(factory) if !factory.r#async && !factory.generator => {
            match factory.body.as_ref().map(|body| body.statements.as_slice()) {
                Some([Statement::ReturnStatement(statement)]) => statement.argument.as_ref(),
                _ => None,
            }
        }
        _ => None,
    };
    let Some(result) = result else {
        return false;
    };
    let Expression::ConditionalExpression(branch) = result.without_parentheses() else {
        return false;
    };
    let Expression::BinaryExpression(test) = branch.test.without_parentheses() else {
        return false;
    };
    let Expression::UnaryExpression(left) = test.left.without_parentheses() else {
        return false;
    };
    let Expression::StringLiteral(right) = test.right.without_parentheses() else {
        return false;
    };
    // esbuild minifies `typeof require !== "undefined"` to `typeof require < "u"`.
    left.operator.is_typeof()
        && matches!(
            (test.operator, right.value.as_str()),
            (BinaryOperator::StrictInequality, Some("undefined"))
                | (BinaryOperator::LessThan, Some("u"))
        )
        && is_runtime_require(&left.argument, semantic)
        && is_runtime_require(&branch.consequent, semantic)
}
