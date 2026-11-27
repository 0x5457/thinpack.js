// `#[op2]` signatures take their arguments by value — the op ABI, not this
// crate, decides — and hand every op a `&mut v8::PinScope` even when it only
// reads through it.
#![expect(
    clippy::needless_pass_by_value,
    reason = "op2 op arguments are owned by the op ABI"
)]
#![expect(
    clippy::needless_pass_by_ref_mut,
    reason = "`&mut v8::PinScope` is the scope type the op layer hands out"
)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;

/// Contexts created by `vm.createContext`, keyed by the id the JavaScript side
/// stores with each context's global object.
#[derive(Default)]
pub(crate) struct Contexts {
    next_id: i32,
    by_id: HashMap<i32, v8::Global<v8::Context>>,
}

impl Contexts {
    fn add(&mut self, context: v8::Global<v8::Context>) -> i32 {
        self.next_id += 1;
        self.by_id.insert(self.next_id, context);
        self.next_id
    }

    fn get(&self, id: i32) -> Option<v8::Global<v8::Context>> {
        self.by_id.get(&id).cloned()
    }
}

pub(crate) const OPS: &[deno_core::OpDecl] = &[
    op_vm_create_context(),
    op_vm_run_in_context(),
    op_vm_compile_function(),
];

/// Returns `[id, global]`: the handle the JavaScript side stores, and the
/// context's global object to hand back as the context.
#[op2(reentrant)]
fn op_vm_create_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    codegen_strings: bool,
) -> v8::Local<'s, v8::Array> {
    let context = v8::Context::new(scope, v8::ContextOptions::default());
    context.set_allow_generation_from_strings(codegen_strings);
    let id = state
        .borrow_mut()
        .borrow_mut::<Contexts>()
        .add(v8::Global::new(scope, context));

    let out = v8::Array::new(scope, 2);
    out.set_index(scope, 0, v8::Integer::new(scope, id).into());
    out.set_index(scope, 1, context.global(scope).into());
    out
}

/// Evaluates `source` in the context `id` and returns `[threw, value]`: whether
/// the script threw, and its result or the engine's own exception. The flag
/// keeps `throw null` and `throw undefined` apart from a normal return.
#[op2(reentrant)]
fn op_vm_run_in_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: i32,
    #[string] source: &str,
    #[string] filename: &str,
    #[smi] line_offset: i32,
    #[smi] column_offset: i32,
) -> Result<v8::Local<'s, v8::Array>, JsErrorBox> {
    let context = state
        .borrow()
        .borrow::<Contexts>()
        .get(id)
        .ok_or_else(|| JsErrorBox::type_error("Unknown vm context"))?;

    let out = v8::Array::new(scope, 2);
    out.set_index(scope, 0, v8::Boolean::new(scope, false).into());
    out.set_index(scope, 1, v8::null(scope).into());

    let context = v8::Local::new(scope, &context);
    let context_scope = &mut v8::ContextScope::new(scope, context);
    v8::tc_scope!(let tc_scope, context_scope);

    let source = v8::String::new(tc_scope, source).expect("the source fits in a V8 string");
    let filename = v8::String::new(tc_scope, filename).expect("the filename fits in a V8 string");
    let origin = v8::ScriptOrigin::new(
        tc_scope,
        filename.into(),
        line_offset,
        column_offset,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    let result = v8::Script::compile(tc_scope, source, Some(&origin))
        .and_then(|script| script.run(tc_scope));

    if let Some(value) = result {
        out.set_index(tc_scope, 1, value);
    } else if let Some(exception) = tc_scope.exception() {
        out.set_index(tc_scope, 0, v8::Boolean::new(tc_scope, true).into());
        out.set_index(tc_scope, 1, exception);
    }
    Ok(out)
}

/// Compiles a function in `id`'s realm, or in the caller's realm when `id` is
/// zero. Returns `[threw, value]` like `op_vm_run_in_context`.
#[op2(reentrant)]
fn op_vm_compile_function<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: i32,
    #[string] source: &str,
    #[string] filename: &str,
    #[smi] line_offset: i32,
    #[smi] column_offset: i32,
    params: v8::Local<'s, v8::Array>,
    context_extensions: v8::Local<'s, v8::Array>,
) -> Result<v8::Local<'s, v8::Array>, JsErrorBox> {
    let context = if id == 0 {
        scope.get_current_context()
    } else {
        let context = state
            .borrow()
            .borrow::<Contexts>()
            .get(id)
            .ok_or_else(|| JsErrorBox::type_error("Unknown vm context"))?;
        v8::Local::new(scope, &context)
    };

    let mut arguments = Vec::new();
    for index in 0..params.length() {
        let value = params
            .get_index(scope, index)
            .and_then(|value| value.try_into().ok())
            .ok_or_else(|| JsErrorBox::type_error("params must be strings"))?;
        arguments.push(value);
    }
    let mut extensions = Vec::new();
    for index in 0..context_extensions.length() {
        let value = context_extensions
            .get_index(scope, index)
            .and_then(|value| value.try_into().ok())
            .ok_or_else(|| JsErrorBox::type_error("contextExtensions must be objects"))?;
        extensions.push(value);
    }

    let out = v8::Array::new(scope, 2);
    out.set_index(scope, 0, v8::Boolean::new(scope, false).into());
    out.set_index(scope, 1, v8::null(scope).into());

    let context_scope = &mut v8::ContextScope::new(scope, context);
    v8::tc_scope!(let tc_scope, context_scope);

    let source = v8::String::new(tc_scope, source).expect("the source fits in a V8 string");
    let filename = v8::String::new(tc_scope, filename).expect("the filename fits in a V8 string");
    let origin = v8::ScriptOrigin::new(
        tc_scope,
        filename.into(),
        line_offset,
        column_offset,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    let mut source = v8::script_compiler::Source::new(source, Some(&origin));
    let function = v8::script_compiler::compile_function(
        tc_scope,
        &mut source,
        &arguments,
        &extensions,
        v8::script_compiler::CompileOptions::NoCompileOptions,
        v8::script_compiler::NoCacheReason::NoReason,
    );

    if let Some(function) = function {
        out.set_index(tc_scope, 1, function.into());
    } else if let Some(exception) = tc_scope.exception() {
        out.set_index(tc_scope, 0, v8::Boolean::new(tc_scope, true).into());
        out.set_index(tc_scope, 1, exception);
    } else {
        // V8 returns an empty result without an exception for an unparsable
        // parameter list; report it in the parsing realm rather than as null.
        let message = v8::String::new(tc_scope, "Invalid function parameters")
            .expect("the message fits in a V8 string");
        out.set_index(tc_scope, 0, v8::Boolean::new(tc_scope, true).into());
        out.set_index(tc_scope, 1, v8::Exception::syntax_error(tc_scope, message));
    }
    Ok(out)
}
