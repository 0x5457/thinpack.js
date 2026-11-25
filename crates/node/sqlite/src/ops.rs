use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpDecl, OpState, op2, v8};
use deno_error::JsErrorBox;

use crate::backup;
use crate::functions;
use crate::resources::Resources;
use crate::session;
use crate::{authorizer, extras, modules};

pub(crate) const OPS: &[OpDecl] = &[
    op_sqlite_open(),
    op_sqlite_close(),
    op_sqlite_exec(),
    op_sqlite_prepare(),
    op_sqlite_finalize(),
    op_sqlite_parameter_names(),
    op_sqlite_clear_bindings(),
    op_sqlite_bind_null(),
    op_sqlite_bind_integer(),
    op_sqlite_bind_real(),
    op_sqlite_bind_text(),
    op_sqlite_bind_blob(),
    op_sqlite_step(),
    op_sqlite_reset(),
    op_sqlite_columns(),
    op_sqlite_changes(),
    op_sqlite_is_autocommit(),
    op_sqlite_expanded_sql(),
    functions::op_sqlite_create_function(),
    functions::op_sqlite_take_callback_error(),
    session::op_sqlite_session_create(),
    session::op_sqlite_session_changeset(),
    session::op_sqlite_session_close(),
    session::op_sqlite_apply_changeset(),
    backup::op_sqlite_backup(),
    extras::op_sqlite_location(),
    extras::op_sqlite_limit(),
    extras::op_sqlite_set_limit(),
    extras::op_sqlite_serialize(),
    extras::op_sqlite_deserialize(),
    extras::op_sqlite_enable_defensive(),
    extras::op_sqlite_enable_load_extension(),
    extras::op_sqlite_load_extension(),
    extras::op_sqlite_statement_status(),
    extras::op_sqlite_statement_reset_status(),
    authorizer::op_sqlite_set_authorizer(),
    modules::op_sqlite_create_module(),
];

#[op2(fast)]
fn op_sqlite_open(
    state: Rc<RefCell<OpState>>,
    #[string] path: &str,
    read_only: bool,
    foreign_keys: bool,
    double_quoted_strings: bool,
    defensive: bool,
    allow_extension: bool,
    #[smi] timeout: u32,
) -> Result<u32, JsErrorBox> {
    Resources::open(
        &state,
        path,
        read_only,
        foreign_keys,
        double_quoted_strings,
        defensive,
        allow_extension,
        timeout,
    )
}

#[op2(fast)]
fn op_sqlite_close(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    Resources::close(&state, id)
}

#[op2(nofast, reentrant)]
fn op_sqlite_exec(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] sql: &str,
) -> Result<(), JsErrorBox> {
    Resources::exec(scope, &state, id, sql)
}

#[op2(nofast, reentrant)]
fn op_sqlite_prepare(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] connection_id: u32,
    #[string] sql: &str,
    persistent: bool,
) -> Result<u32, JsErrorBox> {
    Resources::prepare(scope, &state, connection_id, sql, persistent)
}

#[op2(fast)]
fn op_sqlite_finalize(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    Resources::finalize(&state, id)
}

#[op2]
#[string]
fn op_sqlite_parameter_names(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    Resources::parameter_names(&state, id)
}

#[op2(fast)]
fn op_sqlite_clear_bindings(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    Resources::clear_bindings(&state, id)
}

#[op2(fast)]
fn op_sqlite_bind_null(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
) -> Result<(), JsErrorBox> {
    Resources::bind_null(&state, id, index)
}

#[op2(fast)]
fn op_sqlite_bind_integer(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    #[bigint] value: i64,
) -> Result<(), JsErrorBox> {
    Resources::bind_integer(&state, id, index, value)
}

#[op2(fast)]
fn op_sqlite_bind_real(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    value: f64,
) -> Result<(), JsErrorBox> {
    Resources::bind_real(&state, id, index, value)
}

#[op2(fast)]
fn op_sqlite_bind_text(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    #[string] value: &str,
) -> Result<(), JsErrorBox> {
    Resources::bind_text(&state, id, index, value)
}

#[op2(fast)]
fn op_sqlite_bind_blob(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    #[buffer(copy)] value: Vec<u8>,
) -> Result<(), JsErrorBox> {
    Resources::bind_blob(&state, id, index, &value)
}

#[op2(reentrant)]
#[buffer]
fn op_sqlite_step(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<Box<[u8]>, JsErrorBox> {
    Resources::step(scope, &state, id)
}

#[op2(fast)]
fn op_sqlite_reset(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    Resources::reset(&state, id)
}

#[op2]
#[buffer]
fn op_sqlite_columns(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    Resources::columns(&state, id)
}

#[op2]
#[buffer]
fn op_sqlite_changes(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    Resources::changes(&state, id)
}

#[op2(fast)]
fn op_sqlite_is_autocommit(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<bool, JsErrorBox> {
    Resources::is_autocommit(&state, id)
}

#[op2]
#[string]
fn op_sqlite_expanded_sql(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    Resources::expanded_sql(&state, id)
}
