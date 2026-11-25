use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;

use crate::resources::Resources;

#[op2(nofast, reentrant)]
pub(crate) fn op_sqlite_create_module(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] name: &str,
    #[string] declaration: &str,
    rows: v8::Local<'_, v8::Value>,
) -> Result<(), JsErrorBox> {
    Resources::create_module(scope, &state, id, name, declaration, rows)
}
