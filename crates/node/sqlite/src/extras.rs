use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

use crate::resources::Resources;

#[op2]
#[string]
pub(crate) fn op_sqlite_location(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] database: &str,
) -> Result<Option<String>, JsErrorBox> {
    Resources::location(&state, id, database)
}

#[op2(fast)]
pub(crate) fn op_sqlite_limit(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    limit: i32,
) -> Result<i32, JsErrorBox> {
    Resources::limit(&state, id, limit)
}

#[op2(fast)]
pub(crate) fn op_sqlite_set_limit(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    limit: i32,
    value: i32,
) -> Result<i32, JsErrorBox> {
    Resources::set_limit(&state, id, limit, value)
}

#[op2]
#[buffer]
pub(crate) fn op_sqlite_serialize(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] database: &str,
) -> Result<Box<[u8]>, JsErrorBox> {
    Resources::serialize(&state, id, database)
}

#[op2(fast)]
pub(crate) fn op_sqlite_deserialize(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[buffer(copy)] data: Vec<u8>,
    #[string] database: &str,
    read_only: bool,
) -> Result<(), JsErrorBox> {
    Resources::deserialize(&state, id, data, database, read_only)
}

#[op2(fast)]
pub(crate) fn op_sqlite_enable_defensive(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    enable: bool,
) -> Result<(), JsErrorBox> {
    Resources::enable_defensive(&state, id, enable)
}

#[op2(fast)]
pub(crate) fn op_sqlite_enable_load_extension(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    enable: bool,
) -> Result<(), JsErrorBox> {
    Resources::enable_load_extension(&state, id, enable)
}

#[op2]
pub(crate) fn op_sqlite_load_extension(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] path: &str,
    #[string] entry: Option<String>,
) -> Result<(), JsErrorBox> {
    Resources::load_extension(&state, id, path, entry)
}

#[op2(fast)]
pub(crate) fn op_sqlite_statement_status(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    counter: i32,
) -> Result<i32, JsErrorBox> {
    Resources::statement_status(&state, id, counter)
}

#[op2(fast)]
pub(crate) fn op_sqlite_statement_reset_status(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    counter: i32,
    all: bool,
) -> Result<(), JsErrorBox> {
    Resources::statement_reset_status(&state, id, counter, all)
}
