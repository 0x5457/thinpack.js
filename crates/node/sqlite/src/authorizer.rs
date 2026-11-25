use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;

use crate::resources::Resources;

#[op2(nofast, reentrant)]
pub(crate) fn op_sqlite_set_authorizer(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    callback: Option<v8::Local<'_, v8::Value>>,
) -> Result<(), JsErrorBox> {
    Resources::set_authorizer(scope, &state, id, callback)
}
