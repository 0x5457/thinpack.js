use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use deno_core::{OpDecl, OpState, op2};
use deno_error::JsErrorBox;
use thinpack_core::worker::{self, WorkerContext, WorkerSpec};

pub(crate) const OPS: &[OpDecl] = &[
    op_worker_channel(),
    op_worker_spawn(),
    op_worker_terminate(),
    op_worker_exit(),
    op_worker_error(),
    op_worker_context(),
    op_worker_data(),
    op_port_send(),
    op_port_take(),
    op_port_wait(),
    op_port_close(),
];

#[op2]
#[string]
fn op_worker_channel() -> String {
    let (a, b) = worker::channel();
    format!("{a}\0{b}")
}

#[op2(fast)]
fn op_worker_spawn(
    #[string] script: String,
    #[buffer(copy)] data: Vec<u8>,
    parent_port: u32,
    parent_peer: u32,
) -> Result<u32, JsErrorBox> {
    let spec = WorkerSpec {
        script: PathBuf::from(script),
        data,
        parent_port,
        parent_peer,
        thread_id: worker::next_thread_id(),
    };
    worker::spawn(spec).map_err(|error| JsErrorBox::generic(error.to_string()))
}

#[op2(fast)]
fn op_worker_terminate(id: u32) {
    worker::terminate(id);
}

#[op2(async(lazy), fast)]
async fn op_worker_exit(id: u32) -> u32 {
    // The exit code follows Node's convention: 0 for a clean worker, 1 when
    // the worker ended with an error (surfaced separately through
    // `op_worker_error`).
    u32::from(!worker::wait_exit(id).await)
}

#[op2]
#[string]
fn op_worker_error(id: u32) -> String {
    worker::worker_error(id)
}

#[op2]
#[string]
#[allow(
    clippy::needless_pass_by_value,
    reason = "op2 exposes instance state as `&mut OpState` or `Rc<RefCell<OpState>>`; this op only reads it"
)]
fn op_worker_context(state: Rc<RefCell<OpState>>) -> String {
    state
        .borrow()
        .try_borrow::<WorkerContext>()
        .map_or_else(String::new, |context| {
            format!("{}\0{}\0{}", context.thread_id, context.port, context.peer)
        })
}

#[op2]
#[buffer]
#[allow(
    clippy::needless_pass_by_value,
    reason = "op2 exposes instance state as `&mut OpState` or `Rc<RefCell<OpState>>`; this op only reads it"
)]
fn op_worker_data(state: Rc<RefCell<OpState>>) -> Vec<u8> {
    state
        .borrow()
        .try_borrow::<WorkerContext>()
        .map_or_else(Vec::new, |context| context.data.clone())
}

#[op2(fast)]
fn op_port_send(destination: u32, #[buffer(copy)] data: Vec<u8>) {
    worker::send(destination, data);
}

#[op2]
#[buffer]
fn op_port_take(port: u32) -> Vec<u8> {
    worker::try_take(port).unwrap_or_default()
}

#[op2(async(lazy), fast)]
#[buffer]
async fn op_port_wait(port: u32) -> Vec<u8> {
    worker::wait(port).await.unwrap_or_default()
}

#[op2(fast)]
fn op_port_close(port: u32) {
    worker::close(port);
}
