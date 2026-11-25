use std::cell::RefCell;
use std::ffi::CString;
use std::rc::Rc;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::ffi;

use super::Resources;
use crate::callbacks;
use crate::errors::{connection_error, error, rusqlite_error};

impl Resources {
    #[expect(
        clippy::too_many_arguments,
        reason = "the `node:sqlite` backup arguments are positional"
    )]
    pub(crate) fn backup<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        state: &Rc<RefCell<OpState>>,
        source: u32,
        path: &str,
        source_name: &str,
        target_name: &str,
        rate: u32,
        progress: Option<v8::Local<'s, v8::Value>>,
    ) -> Result<(), JsErrorBox> {
        let source_name = CString::new(source_name).map_err(|_| {
            error(
                "ERR_INVALID_ARG_VALUE",
                "The database name contains a null byte.",
            )
        })?;
        let target_name = CString::new(target_name).map_err(|_| {
            error(
                "ERR_INVALID_ARG_VALUE",
                "The database name contains a null byte.",
            )
        })?;
        let pages = i32::try_from(rate).unwrap_or(i32::MAX);
        let connection = { state.borrow().borrow::<Self>().connection(source)? };
        let destination = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(rusqlite_error)?;

        // SAFETY: both handles are alive for the whole op, and SQLite keeps the
        // returned backup handle owned by us until `sqlite3_backup_finish`.
        let backup = unsafe {
            ffi::sqlite3_backup_init(
                destination.handle(),
                target_name.as_ptr(),
                connection.handle(),
                source_name.as_ptr(),
            )
        };
        if backup.is_null() {
            return Err(connection_error(&destination));
        }

        let progress = progress.and_then(|value| value.try_cast::<v8::Function>().ok());
        let mut failure = None;
        loop {
            // SAFETY: `backup` was returned by `sqlite3_backup_init` on these
            // connections and is not finished yet.
            let rc = unsafe { ffi::sqlite3_backup_step(backup, pages) };
            if rc == ffi::SQLITE_DONE {
                break;
            }
            if rc != ffi::SQLITE_OK {
                failure = Some(connection_error(&connection));
                break;
            }
            if let Some(progress) = progress
                && !report(scope, state, progress, backup)
            {
                failure = Some(error(
                    "ERR_SQLITE_ERROR",
                    "the backup progress callback failed",
                ));
                break;
            }
        }
        // SAFETY: `backup` is finished exactly once, after the step loop.
        let finished = unsafe { ffi::sqlite3_backup_finish(backup) };
        if failure.is_none() && finished != ffi::SQLITE_OK {
            failure = Some(connection_error(&destination));
        }
        callbacks::finish(state, failure)
    }
}

fn report<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &Rc<RefCell<OpState>>,
    progress: v8::Local<'s, v8::Function>,
    backup: *mut ffi::sqlite3_backup,
) -> bool {
    let info = v8::Object::new(scope);
    // SAFETY: `backup` is still live; the op only finishes it after the progress
    // callback returns.
    let total = f64::from(unsafe { ffi::sqlite3_backup_pagecount(backup) });
    // SAFETY: same live handle.
    let remaining = f64::from(unsafe { ffi::sqlite3_backup_remaining(backup) });
    for (key, value) in [("totalPages", total), ("remainingPages", remaining)] {
        let Some(key) = v8::String::new(scope, key) else {
            return false;
        };
        if info
            .create_data_property(scope, key.into(), v8::Number::new(scope, value).into())
            .is_none()
        {
            return false;
        }
    }
    callbacks::invoke(
        scope,
        state,
        progress,
        v8::undefined(scope).into(),
        &[info.into()],
    )
    .is_some()
}
