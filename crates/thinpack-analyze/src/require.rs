use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use oxc_ast::AstKind;
use oxc_ast::ast::{BinaryOperator, Expression, IdentifierReference, Statement};
use oxc_semantic::{IsGlobalReference, Scoping, Semantic, SymbolId};
use oxc_str::JSStr;

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
    mut add: impl FnMut(JSStr<'a>),
) -> bool {
    let argument = argument.without_parentheses();
    if let Expression::StringLiteral(literal) = argument {
        add(literal.value);
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
            Expression::StringLiteral(literal) => add(literal.value),
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
