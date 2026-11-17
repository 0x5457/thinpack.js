use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpDecl, OpState, op2};
use thinpack_core::Assets;

pub(crate) const OPS: &[OpDecl] = &[op_sea_is_sea(), op_sea_asset_keys(), op_sea_asset()];

#[op2(fast)]
#[allow(
    clippy::needless_pass_by_value,
    reason = "op2 exposes instance state as `&mut OpState` or `Rc<RefCell<OpState>>`; these ops only read it"
)]
fn op_sea_is_sea(state: Rc<RefCell<OpState>>) -> bool {
    state.borrow().borrow::<Assets>().is_packed()
}

#[op2]
#[string]
#[allow(
    clippy::needless_pass_by_value,
    reason = "op2 exposes instance state as `&mut OpState` or `Rc<RefCell<OpState>>`; these ops only read it"
)]
fn op_sea_asset_keys(state: Rc<RefCell<OpState>>) -> String {
    let state = state.borrow();
    let assets = state.borrow::<Assets>();
    let keys: Vec<&str> = assets.keys().collect();
    keys.join("\0")
}

#[op2]
#[arraybuffer]
#[allow(
    clippy::needless_pass_by_value,
    reason = "op2 exposes instance state as `&mut OpState` or `Rc<RefCell<OpState>>`; these ops only read it"
)]
fn op_sea_asset(state: Rc<RefCell<OpState>>, #[string] key: &str) -> Vec<u8> {
    let state = state.borrow();
    let assets = state.borrow::<Assets>();
    assets.get(key).unwrap_or(&[]).to_vec()
}
