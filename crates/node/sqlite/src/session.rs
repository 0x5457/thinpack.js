use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;

use crate::resources::Resources;

#[op2]
pub(crate) fn op_sqlite_session_create(
    state: Rc<RefCell<OpState>>,
    #[smi] database: u32,
    #[string] database_name: &str,
    #[string] table: Option<String>,
) -> Result<u32, JsErrorBox> {
    Resources::create_session(&state, database, database_name, table)
}

#[op2]
#[buffer]
pub(crate) fn op_sqlite_session_changeset(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    patchset: bool,
) -> Result<Box<[u8]>, JsErrorBox> {
    Resources::session_changeset(&state, id, patchset)
}

#[op2(fast)]
pub(crate) fn op_sqlite_session_close(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<(), JsErrorBox> {
    Resources::close_session(&state, id)
}

#[op2(nofast, reentrant)]
pub(crate) fn op_sqlite_apply_changeset<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] database: u32,
    #[buffer(copy)] changeset: Vec<u8>,
    filter: Option<v8::Local<'s, v8::Value>>,
    conflict: Option<v8::Local<'s, v8::Value>>,
) -> Result<bool, JsErrorBox> {
    Resources::apply_changeset(scope, &state, database, changeset, filter, conflict)
}
