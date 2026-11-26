use deno_core::{OpDecl, op2};
use deno_error::JsErrorBox;

use thinpack_child::{ChildError, Spawn, Stdio, run_to_completion};

pub(crate) const OPS: &[OpDecl] = &[op_child_spawn(), op_child_spawn_sync()];

/// The `code\0detail` encoding `node:thinpack/internal/errors` decodes.
#[expect(
    clippy::needless_pass_by_value,
    reason = "`ChildError` is a `map_err` sink; taking a reference would force a closure at every call site"
)]
fn wire_error(error: ChildError) -> JsErrorBox {
    JsErrorBox::new("Error", error.to_wire())
}

/// Spawns the child and returns `id\0pid\0channel\0fd`.
///
/// `channel` and `fd` are `-1` when the stdio array has no IPC slot.
#[op2]
#[string]
fn op_child_spawn(
    #[string] argv: &str,
    #[string] env: &str,
    #[string] cwd: &str,
    #[string] stdio: &str,
    ipc: i32,
) -> Result<String, JsErrorBox> {
    let request = Spawn::parse(argv, env, cwd).map_err(wire_error)?;
    let stdio = Stdio::parse(stdio).map_err(wire_error)?;
    if ipc < 0 {
        let spawned = thinpack_child::spawn(&request, stdio, None).map_err(wire_error)?;
        return Ok(format!("{}\0{}\0-1\0-1", spawned.id, spawned.pid));
    }
    let spawned =
        thinpack_child::channel::spawn_with_channel(request, stdio, ipc).map_err(wire_error)?;
    Ok(format!(
        "{}\0{}\0{}\0{}",
        spawned.spawned.id, spawned.spawned.pid, spawned.channel, spawned.fd
    ))
}

/// Runs a child to completion and returns `status`, `signal`, `pid` and both output lengths
/// as little-endian `i32`/`u32` words followed by the outputs; the JavaScript side decodes
/// the same layout.
#[op2]
#[buffer]
fn op_child_spawn_sync(
    #[string] argv: &str,
    #[string] env: &str,
    #[string] cwd: &str,
    #[string] stdio: &str,
    #[buffer(copy)] input: Vec<u8>,
) -> Result<Box<[u8]>, JsErrorBox> {
    let request = Spawn::parse(argv, env, cwd).map_err(wire_error)?;
    let stdio = Stdio::parse(stdio).map_err(wire_error)?;
    let completed = run_to_completion(&request, stdio, input).map_err(wire_error)?;

    let mut payload = Vec::with_capacity(20 + completed.stdout.len() + completed.stderr.len());
    payload.extend_from_slice(&completed.exited.code.unwrap_or(-1).to_le_bytes());
    payload.extend_from_slice(&completed.exited.signal.to_le_bytes());
    payload.extend_from_slice(&completed.pid.to_le_bytes());
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spawnSync result header encodes both lengths as u32, so a longer output cannot be represented on the wire"
    )]
    let stdout_length = completed.stdout.len() as u32;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spawnSync result header encodes both lengths as u32, so a longer output cannot be represented on the wire"
    )]
    let stderr_length = completed.stderr.len() as u32;
    payload.extend_from_slice(&stdout_length.to_le_bytes());
    payload.extend_from_slice(&stderr_length.to_le_bytes());
    payload.extend_from_slice(&completed.stdout);
    payload.extend_from_slice(&completed.stderr);
    Ok(payload.into_boxed_slice())
}
