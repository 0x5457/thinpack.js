use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;

use crate::ChildError;
use crate::table::{self, Stream};

pub(crate) const OPS: &[OpDecl] = &[
    op_child_read(),
    op_child_write(),
    op_child_close_stdin(),
    op_child_wait(),
    op_child_kill(),
];

/// The `code\0detail` encoding `node:thinpack/internal/errors` decodes.
#[expect(
    clippy::needless_pass_by_value,
    reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
)]
fn wire_error(error: ChildError) -> JsErrorBox {
    JsErrorBox::new("Error", error.to_wire())
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_child_read(id: u32, which: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let stream = if which == 1 {
        Stream::Stdout
    } else {
        Stream::Stderr
    };
    table::read(id, stream).await.map_err(wire_error)
}

#[op2(async(lazy), fast)]
async fn op_child_write(id: u32, #[buffer(copy)] data: Vec<u8>) -> Result<u32, JsErrorBox> {
    table::write(id, &data).await.map_err(wire_error)
}

#[op2(async(lazy), fast)]
async fn op_child_close_stdin(id: u32) -> Result<(), JsErrorBox> {
    table::close_stdin(id).await.map_err(wire_error)
}

#[op2(async(lazy), fast)]
#[string]
async fn op_child_wait(id: u32) -> Result<String, JsErrorBox> {
    table::wait(id)
        .await
        .map(|exited| exited.to_wire())
        .map_err(wire_error)
}

#[op2(fast)]
fn op_child_kill(id: u32, signal: i32) -> Result<(), JsErrorBox> {
    table::kill(id, signal).map_err(wire_error)
}
