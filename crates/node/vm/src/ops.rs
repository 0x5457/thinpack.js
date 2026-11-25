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

pub(crate) const OPS: &[deno_core::OpDecl] = &[op_vm_create_context(), op_vm_run_in_context()];

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

/// Evaluates `source` in the context `id` and returns `[value, error]` so the
/// caller can rethrow the engine's own exception, including a compile error.
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
    let null = v8::null(scope);
    out.set_index(scope, 0, null.into());
    out.set_index(scope, 1, null.into());

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
        out.set_index(tc_scope, 0, value);
    } else if let Some(exception) = tc_scope.exception() {
        out.set_index(tc_scope, 1, exception);
    }
    Ok(out)
}
