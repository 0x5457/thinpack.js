use std::ffi::{CStr, c_int};

use deno_error::JsErrorBox;
use rusqlite::Connection;
use rusqlite::ffi;

pub(crate) fn error(code: &str, message: &str) -> JsErrorBox {
    JsErrorBox::new("Error", format!("{code}\0{message}"))
}

pub(crate) fn sqlite_error(errcode: c_int, message: &str) -> JsErrorBox {
    JsErrorBox::new(
        "Error",
        format!(
            "ERR_SQLITE_ERROR\0{message}\0{errcode}\0{}",
            errstr(errcode)
        ),
    )
}

pub(crate) fn errstr(errcode: c_int) -> String {
    // SAFETY: `sqlite3_errstr` returns a pointer to a static NUL-terminated
    // string that stays valid for the rest of the process.
    unsafe { CStr::from_ptr(ffi::sqlite3_errstr(errcode)) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn connection_error(connection: &Connection) -> JsErrorBox {
    // SAFETY: the connection is alive for as long as the returned handle, and
    // the handle is only used for the duration of this call.
    let handle = unsafe { connection.handle() };
    // SAFETY: `handle` is a valid connection, so the extended error code reads
    // the connection's own error state.
    let errcode = unsafe { ffi::sqlite3_extended_errcode(handle) };
    // SAFETY: `sqlite3_errmsg` returns a NUL-terminated string owned by the
    // connection; it is copied out before the connection can be closed.
    let message = unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(handle)) }
        .to_string_lossy()
        .into_owned();
    sqlite_error(errcode, &message)
}

pub(crate) fn rusqlite_error(cause: rusqlite::Error) -> JsErrorBox {
    match cause {
        rusqlite::Error::SqliteFailure(code, message) => sqlite_error(
            code.extended_code,
            &message.unwrap_or_else(|| errstr(code.extended_code)),
        ),
        other => sqlite_error(ffi::SQLITE_ERROR, &other.to_string()),
    }
}
