use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use super::{Authorizer, Resources};
use crate::callbacks;
use crate::errors::{connection_error, error};

impl Resources {
    pub(crate) fn set_authorizer(
        scope: &mut v8::PinScope<'_, '_>,
        state: &Rc<RefCell<OpState>>,
        id: u32,
        callback: Option<v8::Local<'_, v8::Value>>,
    ) -> Result<(), JsErrorBox> {
        let Some(callback) = callback else {
            return clear(state, id);
        };
        let function = callback.try_cast::<v8::Function>().map_err(|_| {
            error(
                "ERR_INVALID_ARG_TYPE",
                "The \"callback\" argument must be a function or null.",
            )
        })?;
        let connection = { state.borrow().borrow::<Self>().connection(id)? };
        // SAFETY: the connection handle is alive here; the old authorizer is detached
        // before its box is dropped below.
        unsafe {
            ffi::sqlite3_set_authorizer(connection.handle(), None, ptr::null_mut());
        }
        let mut authorizer = Box::new(Authorizer {
            function: v8::Global::new(scope, function),
        });
        // SAFETY: `authorizer` is a fresh box that is either handed to SQLite (and
        // freed by `Resources` on the next replacement or connection close) or
        // dropped below, never both.
        let rc = unsafe {
            ffi::sqlite3_set_authorizer(
                connection.handle(),
                Some(authorize),
                (&raw mut *authorizer).cast::<c_void>(),
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(connection_error(&connection));
        }
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .authorizers
            .insert(id, authorizer);
        Ok(())
    }
}

fn clear(state: &Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
    let connection = { state.borrow().borrow::<Resources>().connection(id)? };
    // SAFETY: the connection handle is alive here, and clearing the authorizer
    // takes no user data, so the callback cannot run after this returns.
    unsafe { ffi::sqlite3_set_authorizer(connection.handle(), None, std::ptr::null_mut()) };
    state
        .borrow_mut()
        .borrow_mut::<Resources>()
        .authorizers
        .remove(&id);
    Ok(())
}

unsafe extern "C" fn authorize(
    user_data: *mut c_void,
    action: c_int,
    arg1: *const c_char,
    arg2: *const c_char,
    database: *const c_char,
    trigger: *const c_char,
) -> c_int {
    // SAFETY: `user_data` is the pointer registered by `set_authorizer` for this
    // connection; the box stays alive until the authorizer is replaced or the
    // connection is closed, both of which happen outside a callback.
    let authorizer = unsafe { &*(user_data as *const Authorizer) };
    let text = |value: *const c_char| {
        if value.is_null() {
            None
        } else {
            Some(
                // SAFETY: the null case is handled above, and SQLite passes
                // NUL-terminated strings that stay valid for the whole call.
                unsafe { CStr::from_ptr(value) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    };
    let arguments = (
        action,
        text(arg1),
        text(arg2),
        text(database),
        text(trigger),
    );
    let outcome = callbacks::with_frame(|scope, state| {
        let to_value = |text: Option<String>| {
            text.map_or_else(
                || v8::null(scope).into(),
                |text| {
                    v8::String::new(scope, &text)
                        .map_or_else(|| v8::undefined(scope).into(), Into::into)
                },
            )
        };
        let values = [
            v8::Integer::new(scope, arguments.0).into(),
            to_value(arguments.1),
            to_value(arguments.2),
            to_value(arguments.3),
            to_value(arguments.4),
        ];
        callbacks::invoke(
            scope,
            state,
            v8::Local::new(scope, &authorizer.function),
            v8::undefined(scope).into(),
            &values,
        )
        .and_then(|value| value.int32_value(scope))
    });
    outcome.flatten().unwrap_or(ffi::SQLITE_DENY)
}
