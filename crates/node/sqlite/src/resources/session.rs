use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use super::{Resources, Session};
use crate::callbacks;
use crate::errors::{connection_error, error};

impl Resources {
    pub(crate) fn create_session(
        state: &Rc<RefCell<OpState>>,
        database: u32,
        database_name: &str,
        table: Option<String>,
    ) -> Result<u32, JsErrorBox> {
        let connection = { state.borrow().borrow::<Self>().connection(database)? };
        let database_name = CString::new(database_name).map_err(|_| {
            error(
                "ERR_INVALID_ARG_VALUE",
                "The database name contains a null byte.",
            )
        })?;
        let mut raw = ptr::null_mut();
        // SAFETY: the connection is alive and `database_name` is NUL-terminated; the
        // returned session handle is owned by us and deleted by `Session::drop`.
        let rc = unsafe {
            ffi::sqlite3session_create(connection.handle(), database_name.as_ptr(), &raw mut raw)
        };
        if rc != ffi::SQLITE_OK {
            return Err(connection_error(&connection));
        }
        let table = table
            .map(|table| {
                CString::new(table).map_err(|_| {
                    error(
                        "ERR_INVALID_ARG_VALUE",
                        "The table name contains a null byte.",
                    )
                })
            })
            .transpose()?;
        // SAFETY: `raw` is the session handle created above, and the table name is
        // NUL-terminated or null.
        let rc = unsafe {
            ffi::sqlite3session_attach(
                raw,
                table.as_ref().map_or(ptr::null(), |table| table.as_ptr()),
            )
        };
        if rc != ffi::SQLITE_OK {
            // SAFETY: the session was created above and is not registered anywhere
            // yet, so this is the only owner.
            unsafe { ffi::sqlite3session_delete(raw) };
            return Err(connection_error(&connection));
        }

        let mut op_state = state.borrow_mut();
        Ok(op_state
            .borrow_mut::<Self>()
            .add_session(Session { connection, raw }))
    }

    pub(crate) fn session_changeset(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        patchset: bool,
    ) -> Result<Box<[u8]>, JsErrorBox> {
        let mut op_state = state.borrow_mut();
        let resources = op_state.borrow_mut::<Self>();
        let session = resources
            .sessions
            .get(&id)
            .ok_or_else(|| error("ERR_INVALID_STATE", "session is not open"))?;
        let connection = session.connection.clone();
        let mut length: c_int = 0;
        let mut bytes: *mut c_void = ptr::null_mut();
        // SAFETY: `session.raw` is the live session handle; SQLite writes a buffer of
        // `length` bytes that is freed below.
        let rc = unsafe {
            if patchset {
                ffi::sqlite3session_patchset(session.raw, &raw mut length, &raw mut bytes)
            } else {
                ffi::sqlite3session_changeset(session.raw, &raw mut length, &raw mut bytes)
            }
        };
        if rc != ffi::SQLITE_OK {
            return Err(connection_error(&connection));
        }
        let changeset = callbacks::slice(bytes.cast::<u8>(), length).to_vec();
        // SAFETY: `bytes` is the buffer SQLite allocated for this changeset and it has
        // been copied out; it is freed exactly once here.
        unsafe { ffi::sqlite3_free(bytes) };
        Ok(changeset.into_boxed_slice())
    }

    pub(crate) fn close_session(state: &Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        if op_state.borrow_mut::<Self>().sessions.remove(&id).is_none() {
            return Err(error("ERR_INVALID_STATE", "session is not open"));
        }
        Ok(())
    }

    pub(crate) fn apply_changeset(
        scope: &mut v8::PinScope<'_, '_>,
        state: &Rc<RefCell<OpState>>,
        database: u32,
        changeset: Vec<u8>,
        filter: Option<v8::Local<'_, v8::Value>>,
        conflict: Option<v8::Local<'_, v8::Value>>,
    ) -> Result<bool, JsErrorBox> {
        let connection = { state.borrow().borrow::<Self>().connection(database)? };
        let length = c_int::try_from(changeset.len())
            .map_err(|_| error("ERR_INVALID_ARG_VALUE", "The changeset is too large."))?;
        let filter = filter
            .and_then(|value| value.try_cast::<v8::Function>().ok())
            .map(|function| v8::Global::new(scope, function));
        let on_conflict = conflict
            .and_then(|value| value.try_cast::<v8::Function>().ok())
            .map(|function| v8::Global::new(scope, function));
        let context = ApplyContext {
            filter,
            conflict: on_conflict,
        };
        let has_filter = context.filter.is_some();
        let context = (&raw const context).cast_mut().cast::<c_void>();
        let mut bytes = changeset;

        let rc = {
            let _installed = callbacks::install(scope, state);
            // SAFETY: `context` points at the `ApplyContext` local, which outlives the
            // synchronous apply call that is the only consumer of the pointer.
            unsafe {
                ffi::sqlite3changeset_apply(
                    connection.handle(),
                    length,
                    bytes.as_mut_ptr().cast::<c_void>(),
                    if has_filter { Some(filter_table) } else { None },
                    Some(conflict_handler),
                    context,
                )
            }
        };
        match rc {
            ffi::SQLITE_OK => callbacks::finish(state, None).map(|()| true),
            ffi::SQLITE_ABORT | ffi::SQLITE_CONSTRAINT => {
                callbacks::finish(state, None).map(|()| false)
            }
            _ => callbacks::finish(state, Some(connection_error(&connection))).map(|()| false),
        }
    }
}

struct ApplyContext {
    filter: Option<v8::Global<v8::Function>>,
    conflict: Option<v8::Global<v8::Function>>,
}

unsafe extern "C" fn filter_table(context: *mut c_void, table: *const c_char) -> c_int {
    // SAFETY: `context` is the `ApplyContext` of the in-flight
    // `sqlite3changeset_apply` call, which outlives this callback.
    let context = unsafe { &*context.cast::<ApplyContext>() };
    let Some(filter) = context.filter.as_ref() else {
        return 1;
    };
    // SAFETY: SQLite passes a NUL-terminated table name valid for this call.
    let table = unsafe { CStr::from_ptr(table) }
        .to_string_lossy()
        .into_owned();
    let applied = callbacks::with_frame(|scope, state| -> Option<bool> {
        let name = v8::String::new(scope, &table)?;
        let returned = callbacks::invoke(
            scope,
            state,
            v8::Local::new(scope, filter),
            v8::undefined(scope).into(),
            &[name.into()],
        )?;
        Some(returned.boolean_value(scope))
    });
    i32::from(matches!(applied, Some(Some(true))))
}

unsafe extern "C" fn conflict_handler(
    context: *mut c_void,
    conflict_type: c_int,
    _iterator: *mut ffi::sqlite3_changeset_iter,
) -> c_int {
    // SAFETY: `context` is the `ApplyContext` of the in-flight
    // `sqlite3changeset_apply` call, which outlives this callback.
    let context = unsafe { &*context.cast::<ApplyContext>() };
    let Some(function) = context.conflict.as_ref() else {
        return ffi::SQLITE_CHANGESET_ABORT;
    };
    let outcome = callbacks::with_frame(|scope, state| {
        callbacks::invoke(
            scope,
            state,
            v8::Local::new(scope, function),
            v8::undefined(scope).into(),
            &[v8::Integer::new(scope, conflict_type).into()],
        )
        .and_then(|value| value.int32_value(scope))
    });
    outcome.flatten().unwrap_or(ffi::SQLITE_CHANGESET_ABORT)
}
