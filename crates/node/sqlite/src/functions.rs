use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;

use crate::callbacks;
use crate::resources::Resources;

#[op2(nofast, reentrant)]
pub(crate) fn op_sqlite_create_function<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] database: u32,
    #[string] name: &str,
    #[smi] arguments: i32,
    #[smi] kind: u32,
    deterministic: bool,
    direct_only: bool,
    big_int: bool,
    values: v8::Local<'s, v8::Array>,
) -> Result<(), JsErrorBox> {
    Resources::create_function(
        scope,
        &state,
        database,
        name,
        arguments,
        kind,
        deterministic,
        direct_only,
        big_int,
        values,
    )
}

#[op2(reentrant)]
pub(crate) fn op_sqlite_take_callback_error<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
) -> v8::Local<'s, v8::Value> {
    callbacks::take_pending(scope, &state).unwrap_or_else(|| v8::undefined(scope).into())
}
