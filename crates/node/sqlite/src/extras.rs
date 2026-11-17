use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::rc::Rc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use crate::errors::{connection_error, error};
use crate::ops::{Handles, check, connection_of, statement};

#[op2]
#[string]
pub(crate) fn op_sqlite_location(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] database: &str,
) -> Result<Option<String>, JsErrorBox> {
    let connection = connection_of(&state, id)?;
    let name = CString::new(database).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The database name contains a null byte.",
        )
    })?;
    // SAFETY: the connection is alive for the whole op and `name` is
    // NUL-terminated, so the returned filename stays valid until the schema is
    // changed or the connection is closed.
    let path = unsafe { ffi::sqlite3_db_filename(connection.handle(), name.as_ptr()) };
    if path.is_null() {
        return Ok(None);
    }
    // SAFETY: the null case is handled above; SQLite owns the NUL-terminated
    // filename for at least as long as the connection is open.
    let path = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    Ok((!path.is_empty()).then_some(path))
}

#[op2(fast)]
pub(crate) fn op_sqlite_limit(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    limit: i32,
) -> Result<i32, JsErrorBox> {
    let connection = connection_of(&state, id)?;
    // SAFETY: the connection is alive for the whole op; -1 only queries the limit.
    Ok(unsafe { ffi::sqlite3_limit(connection.handle(), limit, -1) })
}

#[op2(fast)]
pub(crate) fn op_sqlite_set_limit(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    limit: i32,
    value: i32,
) -> Result<i32, JsErrorBox> {
    let connection = connection_of(&state, id)?;
    // SAFETY: the connection is alive for the whole op; SQLite validates `limit`.
    Ok(unsafe { ffi::sqlite3_limit(connection.handle(), limit, value) })
}

#[op2]
#[buffer]
pub(crate) fn op_sqlite_serialize(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] database: &str,
) -> Result<Box<[u8]>, JsErrorBox> {
    let connection = connection_of(&state, id)?;
    let name = CString::new(database).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The database name contains a null byte.",
        )
    })?;
    let mut length: ffi::sqlite3_int64 = 0;
    // SAFETY: the connection is alive and `name` is NUL-terminated; the returned
    // buffer is owned by us and released with `sqlite3_free` below.
    let bytes =
        unsafe { ffi::sqlite3_serialize(connection.handle(), name.as_ptr(), &raw mut length, 0) };
    if bytes.is_null() {
        return Err(connection_error(&connection));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`sqlite3_serialize` reports a non-negative length that fits in a usize"
    )]
    let length = length as usize;
    // SAFETY: `sqlite3_serialize` returned a buffer of exactly `length` bytes.
    let payload = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), length) };
    let copy = payload.to_vec().into_boxed_slice();
    // SAFETY: `bytes` is the buffer allocated by `sqlite3_serialize` and has been
    // copied out; it is freed exactly once here.
    unsafe { ffi::sqlite3_free(bytes.cast()) };
    Ok(copy)
}

#[op2(fast)]
pub(crate) fn op_sqlite_deserialize(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[buffer(copy)] data: Vec<u8>,
    #[string] database: &str,
    read_only: bool,
) -> Result<(), JsErrorBox> {
    let connection = connection_of(&state, id)?;
    let name = CString::new(database).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The database name contains a null byte.",
        )
    })?;
    #[expect(
        clippy::cast_possible_wrap,
        reason = "a Vec is never longer than isize::MAX"
    )]
    let size = data.len() as ffi::sqlite3_int64;
    #[expect(
        clippy::cast_sign_loss,
        reason = "`size` is a Vec length, so it cannot be negative"
    )]
    let allocation = size as u64;
    // SAFETY: `sqlite3_malloc64` returns either null or an allocation of
    // `allocation` bytes, released below or by SQLITE_DESERIALIZE_FREEONCLOSE.
    let buffer = unsafe { ffi::sqlite3_malloc64(allocation) }.cast::<u8>();
    if buffer.is_null() {
        return Err(error("ERR_SQLITE_ERROR", "out of memory"));
    }
    // SAFETY: `buffer` is an uninitialized allocation of exactly `data.len()`
    // bytes, and `data` is that long and does not overlap it.
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len());
    }
    let mut flags = ffi::SQLITE_DESERIALIZE_FREEONCLOSE | ffi::SQLITE_DESERIALIZE_RESIZEABLE;
    if read_only {
        flags |= ffi::SQLITE_DESERIALIZE_READONLY;
    }
    // SAFETY: the connection is alive, `name` is NUL-terminated, and `buffer` is a
    // fresh allocation that SQLite adopts unless the call fails.
    let rc = unsafe {
        ffi::sqlite3_deserialize(
            connection.handle(),
            name.as_ptr(),
            buffer,
            size,
            size,
            flags,
        )
    };
    if rc != ffi::SQLITE_OK {
        // SAFETY: the failed call left `buffer` unadopted, so it is still ours.
        unsafe { ffi::sqlite3_free(buffer.cast()) };
        return Err(connection_error(&connection));
    }
    Ok(())
}

#[op2(fast)]
pub(crate) fn op_sqlite_enable_defensive(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    enable: bool,
) -> Result<(), JsErrorBox> {
    let connection = connection_of(&state, id)?;
    // SAFETY: the connection is alive for the whole op and the out-parameter is
    // null, which `SQLITE_DBCONFIG_DEFENSIVE` accepts.
    let rc = unsafe {
        ffi::sqlite3_db_config(
            connection.handle(),
            ffi::SQLITE_DBCONFIG_DEFENSIVE,
            i32::from(enable),
            std::ptr::null_mut::<i32>(),
        )
    };
    check(rc, &connection)
}

#[op2(fast)]
pub(crate) fn op_sqlite_enable_load_extension(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    enable: bool,
) -> Result<(), JsErrorBox> {
    let connection = connection_of(&state, id)?;
    // SAFETY: the connection is alive for the whole op.
    let rc = unsafe { ffi::sqlite3_enable_load_extension(connection.handle(), i32::from(enable)) };
    check(rc, &connection)
}

#[op2]
pub(crate) fn op_sqlite_load_extension(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] path: &str,
    #[string] entry: Option<String>,
) -> Result<(), JsErrorBox> {
    let connection = connection_of(&state, id)?;
    let path = CString::new(path).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The extension path contains a null byte.",
        )
    })?;
    let entry = entry.map(CString::new).transpose().map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The entry point contains a null byte.",
        )
    })?;
    let mut message: *mut c_char = ptr::null_mut();
    // SAFETY: the connection is alive, both name arguments are NUL-terminated or
    // null, and `message` is only written by SQLite.
    let rc = unsafe {
        ffi::sqlite3_load_extension(
            connection.handle(),
            path.as_ptr(),
            entry.as_ref().map_or(ptr::null(), |entry| entry.as_ptr()),
            &raw mut message,
        )
    };
    let detail = if message.is_null() {
        String::new()
    } else {
        // SAFETY: the null case is handled above; SQLite returns a NUL-terminated
        // message allocated with `sqlite3_malloc`, freed just below.
        let text = unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: `message` was allocated by `sqlite3_load_extension` and is freed
        // exactly once here.
        unsafe { ffi::sqlite3_free(message.cast::<c_void>()) };
        text
    };
    if rc == ffi::SQLITE_OK {
        Ok(())
    } else if detail.is_empty() {
        Err(error(
            "ERR_LOAD_SQLITE_EXTENSION",
            "failed to load the SQLite extension",
        ))
    } else {
        Err(error("ERR_LOAD_SQLITE_EXTENSION", &detail))
    }
}

#[op2(fast)]
pub(crate) fn op_sqlite_statement_status(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    counter: i32,
) -> Result<i32, JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    // SAFETY: the statement is registered and only finalized by `Handles`; reset
    // is 0, so this only reads the counter.
    Ok(unsafe { ffi::sqlite3_stmt_status(statement.raw, counter, 0) })
}

#[op2(fast)]
pub(crate) fn op_sqlite_statement_reset_status(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    counter: i32,
    all: bool,
) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    if all {
        for counter in 1..=9 {
            // SAFETY: live statement; reset is 1, so this only resets the counter.
            unsafe {
                ffi::sqlite3_stmt_status(statement.raw, counter, 1);
            }
        }
    } else {
        // SAFETY: live statement; reset is 1, so this only resets the counter.
        unsafe {
            ffi::sqlite3_stmt_status(statement.raw, counter, 1);
        }
    }
    Ok(())
}
