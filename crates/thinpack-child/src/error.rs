use std::fmt;

use deno_core::uv_compat;

/// A failed operation on a child process, in the shape both JavaScript surfaces need.
///
/// `code` is the symbolic name `node:child_process` sets on `error.code`; `errno` is the
/// libuv code when the failure came from the operating system, which is what
/// `node:cluster` raises a `UVException` from. Carrying both keeps one error value usable
/// by either surface instead of formatting a wire string inside the layer.
#[derive(Debug)]
pub struct ChildError {
    code: &'static str,
    errno: Option<i32>,
    detail: String,
}

impl ChildError {
    #[must_use]
    pub fn new(code: &'static str, detail: impl fmt::Display) -> Self {
        Self {
            code,
            errno: None,
            detail: detail.to_string(),
        }
    }

    /// Wraps an OS failure under the symbolic code the calling op reports for it.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "`io::Error` is a `map_err` sink; taking a reference would force a closure at every call site"
    )]
    #[must_use]
    pub fn io(code: &'static str, error: std::io::Error) -> Self {
        Self {
            code,
            errno: Some(uv_compat::io_error_to_uv(&error)),
            detail: error.to_string(),
        }
    }

    /// A failed signal: the two answers `kill()` callers distinguish are a process that is
    /// already gone and one this process may not signal.
    #[must_use]
    pub fn signal(error: std::io::Error) -> Self {
        Self::io(signal_code(&error), error)
    }

    /// A failure to start a process, which is the one place the error kind selects the code.
    #[must_use]
    pub fn spawn(error: std::io::Error) -> Self {
        let code = match error.kind() {
            std::io::ErrorKind::NotFound => "ENOENT",
            std::io::ErrorKind::PermissionDenied => "EACCES",
            _ => "EIO",
        };
        Self::io(code, error)
    }

    /// `code\0detail`, the encoding `node:thinpack/internal/errors` decodes.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!("{}\0{}", self.code, self.detail)
    }

    /// `errno\0detail` for a surface that raises a `UVException`, falling back to the
    /// symbolic form when the failure has no errno (an invalid argument, say).
    #[must_use]
    pub fn to_uv_wire(&self) -> String {
        self.errno.map_or_else(
            || self.to_wire(),
            |errno| format!("{errno}\0{}", self.detail),
        )
    }
}

/// The code for a failed `kill`, from the OS error that produced it.
#[cfg(unix)]
fn signal_code(error: &std::io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(libc::ESRCH) => "ESRCH",
        Some(libc::EPERM) => "EPERM",
        _ => "EIO",
    }
}

#[cfg(windows)]
fn signal_code(error: &std::io::Error) -> &'static str {
    match error.raw_os_error() {
        // `OpenProcess` reports a pid that no longer exists as an invalid parameter.
        Some(87) => "ESRCH",
        Some(5) => "EPERM",
        _ => "EIO",
    }
}
