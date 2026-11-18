use std::borrow::Cow;
use std::io::{self, Write};

use deno_core::{Extension, OpDecl, op2};

fn write(is_err: bool, bytes: &[u8]) -> io::Result<()> {
    let mut stream: Box<dyn Write> = if is_err {
        Box::new(io::stderr())
    } else {
        Box::new(io::stdout())
    };
    stream.write_all(bytes)?;
    stream.flush()
}

#[op2(fast)]
pub(crate) fn op_thinpack_io_write(#[string] text: &str, is_err: bool) -> Result<(), io::Error> {
    write(is_err, text.as_bytes())
}

#[op2(fast)]
pub(crate) fn op_thinpack_io_write_bytes(
    #[buffer] bytes: &[u8],
    is_err: bool,
) -> Result<(), io::Error> {
    write(is_err, bytes)
}

#[op2(async(lazy), fast)]
#[buffer]
pub(crate) async fn op_thinpack_io_read_stdin() -> Result<Box<[u8]>, io::Error> {
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0u8; 8192];
    let read = tokio::io::stdin().read(&mut buffer).await?;
    buffer.truncate(read);
    Ok(buffer.into_boxed_slice())
}

#[op2(fast)]
pub(crate) fn op_thinpack_io_isatty(#[smi] fd: i32) -> bool {
    // `uv_guess_handle` is deno_core's libuv-compatible cross-platform check;
    // on Windows it recognizes console handles as well as files and pipes.
    deno_core::uv_compat::uv_guess_handle(fd) == deno_core::uv_compat::uv_handle_type::UV_TTY
}

#[must_use]
pub fn init() -> Extension {
    const OPS: &[OpDecl] = &[
        op_thinpack_io_write(),
        op_thinpack_io_write_bytes(),
        op_thinpack_io_read_stdin(),
        op_thinpack_io_isatty(),
    ];
    Extension {
        name: "thinpack_io",
        ops: Cow::Borrowed(OPS),
        ..Default::default()
    }
}
