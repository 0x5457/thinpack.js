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
    // SAFETY: `isatty` takes the descriptor by value and dereferences nothing,
    // so every `i32` is a valid argument.
    unsafe { libc::isatty(fd) == 1 }
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
