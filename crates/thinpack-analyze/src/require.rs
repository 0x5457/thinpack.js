use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use oxc_ast::AstKind;
use oxc_ast::ast::{
    Argument, BinaryOperator, BindingPattern, CallExpression, Expression, FormalParameters,
    IdentifierReference, ObjectExpression, ObjectPropertyKind, PropertyKey, PropertyKind,
    Statement, StaticMemberExpression,
};
use oxc_semantic::{IsGlobalReference, Reference, ScopeId, Scoping, Semantic, SymbolId};
use oxc_span::{GetSpan, Span};

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

/// The argument index a `Function`-built import factory reads its specifier from.
///
/// `new Function("module", "return import(module)")` creates a function that imports
/// its argument in the module that created it, so a literal call names a module the
/// analyzer can follow. Only the literal body `return import(<parameter>)` is
/// recognized; nothing in it is evaluated.
pub(super) fn late_import(
    callee: &Expression<'_>,
    arguments: &[Argument<'_>],
    semantic: &Semantic<'_>,
) -> Option<usize> {
    if !is_function_constructor(callee, semantic) {
        return None;
    }
    let (body, parameters) = arguments.split_last()?;
    let body = string_literal(body.as_expression()?)?;
    let parameters = parameters
        .iter()
        .map(|argument| string_literal(argument.as_expression()?))
        .collect::<Option<Vec<_>>>()?
        .join(",");
    let body = body.trim();
    parameters.split(',').map(str::trim).position(|parameter| {
        let expected = format!("return import({parameter})");
        body == expected || body.strip_suffix(';').is_some_and(|body| body == expected)
    })
}

/// A stable binding created by a `Function`-built import factory.
pub(super) struct LateImport {
    /// The argument index the factory body imports.
    pub argument: usize,
    /// The constructor expression, so its generic warning can be suppressed.
    pub span: Span,
}

/// The stable bindings (`const load = new Function(...)`) that late-import their
/// argument, with the argument index the body reads.
pub(super) fn late_import_bindings(semantic: &Semantic<'_>) -> BTreeMap<SymbolId, LateImport> {
    semantic
        .scoping()
        .symbol_ids()
        .filter_map(|symbol| {
            // An exported binding can be called from a module this walk cannot see,
            // so keep its construction warning instead of treating it as handled. Only
            // a directly exported declaration qualifies; a factory inside an exported
            // function stays local to its own call graph.
            let declaration = semantic.symbol_declaration(symbol);
            let variable = semantic.nodes().parent_id(declaration.id());
            if matches!(
                semantic.nodes().kind(variable),
                AstKind::VariableDeclaration(_)
            ) && matches!(
                semantic.nodes().parent_kind(variable),
                AstKind::ExportDeclaration(_)
            ) {
                return None;
            }
            let (argument, span) = match initializer(symbol, semantic)? {
                Expression::NewExpression(expression) => (
                    late_import(&expression.callee, &expression.arguments, semantic)?,
                    expression.span,
                ),
                Expression::CallExpression(call) => (
                    late_import(&call.callee, &call.arguments, semantic)?,
                    call.span,
                ),
                _ => return None,
            };
            if !semantic
                .symbol_references(symbol)
                .all(|reference| is_direct_call_callee(reference, semantic))
            {
                return None;
            }
            Some((symbol, LateImport { argument, span }))
        })
        .collect()
}

/// The literal string arguments of every direct call of a same-file function
/// whose binding is neither exported nor reassigned. Each call is a separate
/// binding map, so a parameter supplied with different literals yields one path
/// per call instead of a guessed union. A spread, a non-call reference or a
/// redeclaration leaves the function unresolved, and a parameter without a
/// literal argument stays absent from that call's map.
pub(super) fn literal_calls<'a>(
    symbol: SymbolId,
    parameters: &'a FormalParameters<'a>,
    scope: Option<ScopeId>,
    semantic: &Semantic<'a>,
) -> Option<Vec<BTreeMap<SymbolId, &'a Expression<'a>>>> {
    let scoping = semantic.scoping();
    if parameters.rest.is_some()
        || scope.is_some_and(|scope| scoping.scope_flags(scope).contains_direct_eval())
        || is_exported(symbol, semantic)
        || scoping.symbol_is_mutated(symbol)
        || !scoping.symbol_redeclarations(symbol).is_empty()
    {
        return None;
    }
    let parameters = parameters
        .items
        .iter()
        .map(|parameter| {
            if parameter.initializer.is_some() {
                return None;
            }
            let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
                return None;
            };
            let symbol = binding.symbol_id.get()?;
            (!scoping.symbol_is_mutated(symbol) && scoping.symbol_redeclarations(symbol).is_empty())
                .then_some(symbol)
        })
        .collect::<Option<Vec<_>>>()?;
    let mut calls = Vec::new();
    for reference in semantic.symbol_references(symbol) {
        if !is_direct_call_callee(reference, semantic) {
            return None;
        }
        let AstKind::CallExpression(call) = semantic.nodes().parent_kind(reference.node_id())
        else {
            return None;
        };
        if call
            .arguments
            .iter()
            .any(|argument| matches!(argument, Argument::SpreadElement(_)))
        {
            return None;
        }
        let bindings = parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                let argument = call
                    .arguments
                    .get(index)?
                    .as_expression()?
                    .without_parentheses();
                matches!(argument, Expression::StringLiteral(_)).then_some((*parameter, argument))
            })
            .collect();
        calls.push(bindings);
    }
    (!calls.is_empty()).then_some(calls)
}

fn is_direct_call_callee(reference: &Reference, semantic: &Semantic<'_>) -> bool {
    let AstKind::CallExpression(call) = semantic.nodes().parent_kind(reference.node_id()) else {
        return false;
    };
    let AstKind::IdentifierReference(identifier) =
        semantic.nodes().get_node(reference.node_id()).kind()
    else {
        return false;
    };
    call.callee
        .get_identifier_reference()
        .is_some_and(|callee| std::ptr::eq(callee, identifier))
}

fn is_function_constructor(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    match expression.without_parentheses() {
        Expression::Identifier(identifier) => {
            identifier.name == "Function" && identifier.is_global_reference(semantic.scoping())
        }
        Expression::StaticMemberExpression(member) => {
            member.property.name == "Function"
                && matches!(
                    &member.object,
                    Expression::Identifier(object)
                        if object.name == "globalThis"
                            && object.is_global_reference(semantic.scoping())
                )
        }
        _ => false,
    }
}

fn string_literal<'a>(expression: &'a Expression<'a>) -> Option<&'a str> {
    match expression.without_parentheses() {
        Expression::StringLiteral(literal) => literal.value.as_str(),
        _ => None,
    }
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

/// The string an expression always produces, for the manifest and bin names a
/// launcher reads out of a plain data object.
///
/// Only a literal, a template of static parts, and a property of a plain object
/// literal are accepted. A property read is sound only while no alias or call can
/// mutate or replace the object, so
/// the object binding is limited to `const` initialized by an unambiguous literal
/// whose references are member reads or known direct calls.
pub(super) fn static_string(
    expression: &Expression<'_>,
    semantic: &Semantic<'_>,
) -> Option<String> {
    if semantic.scoping().root_scope_flags().contains_direct_eval() {
        return None;
    }
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
        Expression::StaticMemberExpression(member) => member_value(member, semantic, visiting),
        _ => None,
    }
}

/// A property of a plain object literal or of a parameter that every direct call
/// passes such a literal (or a binding frozen by the same rules).
fn member_value(
    member: &StaticMemberExpression<'_>,
    semantic: &Semantic<'_>,
    visiting: &mut BTreeSet<SymbolId>,
) -> Option<String> {
    let property = member.property.name.as_str();
    let Expression::Identifier(object) = member.object.without_parentheses() else {
        return None;
    };
    let symbol = referenced_symbol(object, semantic.scoping())?;
    if !visiting.insert(symbol) {
        return None;
    }
    let value = if initializer(symbol, semantic).is_some() {
        object_binding(symbol, semantic)
            .and_then(|object| object_value(object, property, semantic, visiting))
    } else {
        parameter_value(symbol, semantic, |argument| {
            object_argument_value(argument, property, semantic, visiting)
        })
    };
    visiting.remove(&symbol);
    value
}

/// A plain object literal: no spread, methods, accessors, computed keys, duplicate
/// keys, or `__proto__`. The remaining properties have one unambiguous value.
fn plain_object<'a>(expression: &'a Expression<'a>) -> Option<&'a ObjectExpression<'a>> {
    let Expression::ObjectExpression(object) = expression.without_parentheses() else {
        return None;
    };
    let mut keys = BTreeSet::new();
    for entry in &object.properties {
        let ObjectPropertyKind::ObjectProperty(property) = entry else {
            return None;
        };
        if property.kind != PropertyKind::Init || property.method || property.computed {
            return None;
        }
        let key = match &property.key {
            PropertyKey::StaticIdentifier(identifier) => identifier.name.as_str(),
            PropertyKey::StringLiteral(literal) => literal.value.as_str()?,
            _ => return None,
        };
        if key == "__proto__" || !keys.insert(key) {
            return None;
        }
    }
    Some(object)
}

fn object_value<'a>(
    object: &'a ObjectExpression<'a>,
    property_name: &str,
    semantic: &Semantic<'a>,
    visiting: &mut BTreeSet<SymbolId>,
) -> Option<String> {
    let property = object.properties.iter().find_map(|entry| {
        let ObjectPropertyKind::ObjectProperty(property) = entry else {
            return None;
        };
        let key = match &property.key {
            PropertyKey::StaticIdentifier(identifier) => identifier.name.as_str(),
            PropertyKey::StringLiteral(literal) => literal.value.as_str()?,
            _ => return None,
        };
        (key == property_name).then_some(property)
    })?;
    static_string_with(&property.value, semantic, visiting)
}

/// A `const` object that no visible reference can mutate, alias, or export.
fn object_binding<'a>(
    symbol: SymbolId,
    semantic: &Semantic<'a>,
) -> Option<&'a ObjectExpression<'a>> {
    let object = plain_object(initializer(symbol, semantic)?)?;
    if is_exported(symbol, semantic) {
        return None;
    }
    semantic
        .symbol_references(symbol)
        .all(|reference| {
            is_member_read(reference, semantic) || is_safe_call_argument(reference, semantic)
        })
        .then_some(object)
}

fn object_argument_value(
    argument: &Expression<'_>,
    property: &str,
    semantic: &Semantic<'_>,
    visiting: &mut BTreeSet<SymbolId>,
) -> Option<String> {
    match argument.without_parentheses() {
        Expression::ObjectExpression(_) => {
            object_value(plain_object(argument)?, property, semantic, visiting)
        }
        Expression::Identifier(identifier) => {
            let symbol = referenced_symbol(identifier, semantic.scoping())?;
            if !visiting.insert(symbol) {
                return None;
            }
            let value = object_binding(symbol, semantic)
                .and_then(|object| object_value(object, property, semantic, visiting));
            visiting.remove(&symbol);
            value
        }
        _ => None,
    }
}

/// The object a parameter receives, when every direct call passes the same plain
/// object and the parameter only reads properties.
fn parameter_value<T: PartialEq>(
    symbol: SymbolId,
    semantic: &Semantic<'_>,
    mut evaluate: impl FnMut(&Expression<'_>) -> Option<T>,
) -> Option<T> {
    let declaration = semantic.symbol_declaration(symbol);
    let parameter = declaration.kind().as_formal_parameter()?;
    if parameter.initializer.is_some()
        || !matches!(&parameter.pattern, BindingPattern::BindingIdentifier(_))
        || !is_constant(symbol, semantic)
        || !references_only_member_reads(symbol, semantic)
    {
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
    let AstKind::ArrowFunctionExpression(function) = nodes.parent_node(parameters.id()).kind()
    else {
        return None;
    };
    let AstKind::VariableDeclarator(binding) = nodes.parent_node(function.node_id.get()).kind()
    else {
        return None;
    };
    let function_symbol = binding.id.get_binding_identifier()?.symbol_id.get()?;
    if arrow_function(function_symbol, semantic).is_none_or(|bound| bound.span != function.span) {
        return None;
    }

    let mut value = None;
    for reference in semantic.symbol_references(function_symbol) {
        let node = nodes.get_node(reference.node_id());
        let AstKind::CallExpression(call) = nodes.parent_node(node.id()).kind() else {
            return None;
        };
        let argument = call_argument(call, index)?;
        let candidate = evaluate(argument)?;
        if value.as_ref().is_some_and(|value| value != &candidate) {
            return None;
        }
        value = Some(candidate);
    }
    value
}

/// The argument at `index`, unless a spread makes the position ambiguous.
fn call_argument<'a>(call: &'a CallExpression<'a>, index: usize) -> Option<&'a Expression<'a>> {
    if call
        .arguments
        .iter()
        .any(|argument| matches!(argument, Argument::SpreadElement(_)))
    {
        return None;
    }
    call.arguments.get(index)?.as_expression()
}

fn is_member_read(reference: &Reference, semantic: &Semantic<'_>) -> bool {
    let nodes = semantic.nodes();
    let AstKind::StaticMemberExpression(member) = nodes.parent_kind(reference.node_id()) else {
        return false;
    };
    !reference.is_write()
        && !reference.flags().is_member_write_target()
        && member
            .object
            .get_identifier_reference()
            .is_some_and(|object| {
                object.span() == nodes.get_node(reference.node_id()).kind().span()
            })
}

/// A read passed straight to a local arrow whose parameter only reads properties.
fn is_safe_call_argument(reference: &Reference, semantic: &Semantic<'_>) -> bool {
    if reference.is_write() {
        return false;
    }
    let nodes = semantic.nodes();
    let AstKind::CallExpression(call) = nodes.parent_kind(reference.node_id()) else {
        return false;
    };
    if call
        .arguments
        .iter()
        .any(|argument| matches!(argument, Argument::SpreadElement(_)))
    {
        return false;
    }
    let span = nodes.get_node(reference.node_id()).kind().span();
    let Some(index) = call.arguments.iter().position(|argument| {
        argument.as_expression().is_some_and(|expression| {
            expression
                .without_parentheses()
                .get_identifier_reference()
                .is_some_and(|identifier| identifier.span() == span)
        })
    }) else {
        return false;
    };
    let Expression::Identifier(callee) = call.callee.without_parentheses() else {
        return false;
    };
    let Some(function_symbol) = referenced_symbol(callee, semantic.scoping()) else {
        return false;
    };
    let Some(function) = arrow_function(function_symbol, semantic) else {
        return false;
    };
    let Some(parameter) = parameter_symbol(function, index) else {
        return false;
    };
    references_only_member_reads(parameter, semantic)
}

/// A local `const` arrow whose references are direct calls only.
fn arrow_function<'a>(
    symbol: SymbolId,
    semantic: &Semantic<'a>,
) -> Option<&'a oxc_ast::ast::ArrowFunctionExpression<'a>> {
    let Expression::ArrowFunctionExpression(function) = initializer(symbol, semantic)? else {
        return None;
    };
    if is_exported(symbol, semantic)
        || !semantic
            .symbol_references(symbol)
            .all(|reference| is_direct_call_callee(reference, semantic))
        || function
            .scope_id
            .get()
            .is_some_and(|scope| semantic.scoping().scope_flags(scope).contains_direct_eval())
    {
        return None;
    }
    Some(function)
}

/// The simple identifier bound at `index`, without a default value.
fn parameter_symbol(
    function: &oxc_ast::ast::ArrowFunctionExpression<'_>,
    index: usize,
) -> Option<SymbolId> {
    let parameter = function.params.items.get(index)?;
    if parameter.initializer.is_some() {
        return None;
    }
    let BindingPattern::BindingIdentifier(binding) = &parameter.pattern else {
        return None;
    };
    binding.symbol_id.get()
}

fn references_only_member_reads(symbol: SymbolId, semantic: &Semantic<'_>) -> bool {
    semantic
        .symbol_references(symbol)
        .all(|reference| is_member_read(reference, semantic))
}

fn is_exported(symbol: SymbolId, semantic: &Semantic<'_>) -> bool {
    let nodes = semantic.nodes();
    let declaration = semantic.symbol_declaration(symbol);
    match declaration.kind() {
        AstKind::VariableDeclarator(_) => {
            let variable = nodes.parent_id(declaration.id());
            matches!(nodes.kind(variable), AstKind::VariableDeclaration(_))
                && matches!(nodes.parent_kind(variable), AstKind::ExportDeclaration(_))
        }
        AstKind::Function(_) => matches!(
            nodes.parent_kind(declaration.id()),
            AstKind::ExportDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
        ),
        _ => false,
    }
}

/// Every expression assigned to a binding: its initializer and each assignment.
/// A computed module path often has no initializer and several assignments, so
/// the literal prefix cannot be read from the declaration alone.
pub(super) fn assigned_values<'a>(
    symbol: SymbolId,
    semantic: &Semantic<'a>,
) -> Vec<&'a Expression<'a>> {
    let mut values = Vec::new();
    if let AstKind::VariableDeclarator(declaration) = semantic.symbol_declaration(symbol).kind()
        && let Some(init) = &declaration.init
    {
        values.push(init.without_parentheses());
    }
    for reference in semantic.symbol_references(symbol) {
        if reference.is_write()
            && let AstKind::AssignmentExpression(assignment) =
                semantic.nodes().parent_kind(reference.node_id())
        {
            values.push(assignment.right.without_parentheses());
        }
    }
    values
}

pub(super) fn initializer<'a>(
    symbol: SymbolId,
    semantic: &Semantic<'a>,
) -> Option<&'a Expression<'a>> {
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

/// The package-manifest specifiers whose loaded object is used to select a bin.
///
/// The selection is only a member access such as `pkg.bin[binName]` inside a
/// computed `require`/`import` target; reading the manifest for its version alone
/// selects nothing.
pub(super) fn selected_bins(
    expression: &Expression<'_>,
    semantic: &Semantic<'_>,
    manifests: &BTreeMap<SymbolId, BTreeSet<String>>,
) -> Vec<(String, String)> {
    let mut selected = BTreeSet::new();
    collect_bin_selections(expression, semantic, manifests, &mut selected);
    selected.into_iter().collect()
}

fn collect_bin_selections(
    expression: &Expression<'_>,
    semantic: &Semantic<'_>,
    manifests: &BTreeMap<SymbolId, BTreeSet<String>>,
    selected: &mut BTreeSet<(String, String)>,
) {
    match expression.without_parentheses() {
        Expression::CallExpression(call) => {
            for argument in &call.arguments {
                if let Some(argument) = argument.as_expression() {
                    collect_bin_selections(argument, semantic, manifests, selected);
                }
            }
        }
        Expression::BinaryExpression(binary) => {
            collect_bin_selections(&binary.left, semantic, manifests, selected);
            collect_bin_selections(&binary.right, semantic, manifests, selected);
        }
        Expression::TemplateLiteral(template) => {
            for expression in &template.expressions {
                collect_bin_selections(expression, semantic, manifests, selected);
            }
        }
        Expression::ComputedMemberExpression(member) => {
            if let Expression::StaticMemberExpression(bin) = member.object.without_parentheses()
                && bin.property.name == "bin"
                && let Some(name) = static_string(&member.expression, semantic)
            {
                add_bin_selection(&bin.object, &name, semantic, manifests, selected);
            }
        }
        Expression::StaticMemberExpression(member) => {
            if let Expression::StaticMemberExpression(bin) = member.object.without_parentheses()
                && bin.property.name == "bin"
            {
                add_bin_selection(
                    &bin.object,
                    member.property.name.as_str(),
                    semantic,
                    manifests,
                    selected,
                );
            }
        }
        _ => {}
    }
}

fn add_bin_selection(
    object: &Expression<'_>,
    name: &str,
    semantic: &Semantic<'_>,
    manifests: &BTreeMap<SymbolId, BTreeSet<String>>,
    selected: &mut BTreeSet<(String, String)>,
) {
    let Expression::Identifier(identifier) = object.without_parentheses() else {
        return;
    };
    let Some(symbol) = referenced_symbol(identifier, semantic.scoping()) else {
        return;
    };
    let Some(specifiers) = manifest_specifiers(symbol, semantic, manifests) else {
        return;
    };
    for specifier in specifiers {
        selected.insert((specifier.clone(), name.to_owned()));
    }
}

/// The manifest a binding was loaded from, either directly or through
/// `const pkg = require(pkgPath)`.
fn manifest_specifiers<'a>(
    symbol: SymbolId,
    semantic: &Semantic<'_>,
    manifests: &'a BTreeMap<SymbolId, BTreeSet<String>>,
) -> Option<&'a BTreeSet<String>> {
    if let Some(specifiers) = manifests.get(&symbol) {
        return Some(specifiers);
    }
    let Expression::CallExpression(call) = initializer(symbol, semantic)? else {
        return None;
    };
    if !is_runtime_require(&call.callee, semantic) {
        return None;
    }
    let Expression::Identifier(identifier) = call
        .arguments
        .first()?
        .as_expression()?
        .without_parentheses()
    else {
        return None;
    };
    let manifest = referenced_symbol(identifier, semantic.scoping())?;
    manifests.get(&manifest)
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
    is_create_require(&call.callee, semantic)
}

/// Whether `callee` is the `createRequire` imported from `node:module`.
fn is_create_require(callee: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    let Some(identifier) = callee.without_parentheses().get_identifier_reference() else {
        return false;
    };
    let Some(symbol) = referenced_symbol(identifier, semantic.scoping()) else {
        return false;
    };
    let declaration = semantic.symbol_declaration(symbol);
    matches!(declaration.kind(), AstKind::ImportSpecifier(specifier)
        if specifier.imported.name() == "createRequire")
        && matches!(semantic.nodes().parent_kind(declaration.id()),
            AstKind::ImportDeclaration(import) if matches!(import.source.value.as_str(), Some("node:module" | "module")))
}

/// Whether `call` is a direct call of the named `createRequire` import with the
/// importing module's own URL as its base:
/// `createRequire(import.meta.url)(<specifier>)`.
///
/// A relative specifier resolves against the file `createRequire` names, so the
/// call is a dependency edge of this module only when the base is this module's
/// `import.meta.url`. Another base, or a same-name helper, is never a direct
/// require; [`importer_url_is_stable`] decides whether that URL is provably
/// unchanged.
pub(super) fn is_importer_require_call(call: &CallExpression<'_>, semantic: &Semantic<'_>) -> bool {
    if !is_create_require(&call.callee, semantic) {
        return false;
    }
    let Some(argument) = call.arguments.first().and_then(Argument::as_expression) else {
        return false;
    };
    let Expression::StaticMemberExpression(member) = argument.without_parentheses() else {
        return false;
    };
    member.property.name == "url" && matches!(&member.object, Expression::ImportMeta(_))
}

/// Whether every `import.meta` in the module is the `.url` member passed straight
/// to a proven `createRequire` call.
///
/// `import.meta.url` is an ordinary writable property, so a syntactic member read
/// is only the module URL while no other expression can write it or expose
/// `import.meta` to code that does. `import.meta` has no symbol or reference for
/// the existing mutation helpers, so every AST use is checked directly: passing the
/// string to a proven `createRequire` copies it and cannot change the URL.
pub(super) fn importer_url_is_stable(semantic: &Semantic<'_>) -> bool {
    let nodes = semantic.nodes();
    nodes.iter().all(|node| {
        if !matches!(node.kind(), AstKind::ImportMeta(_)) {
            return true;
        }
        let AstKind::StaticMemberExpression(member) = nodes.parent_kind(node.id()) else {
            return false;
        };
        member.property.name == "url"
            && matches!(&member.object, Expression::ImportMeta(_))
            && matches!(
                nodes.parent_kind(nodes.parent_id(node.id())),
                AstKind::CallExpression(call) if is_create_require(&call.callee, semantic)
            )
    })
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
