use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::time::Duration;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::ffi;

use crate::callbacks;
use crate::errors::{connection_error, error, errstr, rusqlite_error, sqlite_error};

mod authorizer;

mod backup;

mod extras;

mod functions;

mod modules;

mod session;

mod statement;

/// Owns every SQLite resource the module keeps alive and implements the
/// operations on those resources. The maps stay private so the op layer can
/// only reach them through a domain operation, never a handle lookup.
#[derive(Default)]
pub(crate) struct Resources {
    next_connection: u32,
    next_statement: u32,
    next_session: u32,
    connections: HashMap<u32, Rc<Connection>>,
    statements: HashMap<u32, PreparedStatement>,
    sessions: HashMap<u32, Session>,
    authorizers: HashMap<u32, Box<Authorizer>>,
}

struct PreparedStatement {
    connection: Rc<Connection>,
    raw: *mut ffi::sqlite3_stmt,
    busy: bool,
}

impl Drop for PreparedStatement {
    fn drop(&mut self) {
        // SAFETY: `raw` comes from `sqlite3_prepare_v3` and is finalized exactly
        // once, since the statement is owned solely by `Resources::statements`.
        unsafe { ffi::sqlite3_finalize(self.raw) };
    }
}

struct Session {
    connection: Rc<Connection>,
    raw: *mut ffi::sqlite3_session,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: `raw` comes from `sqlite3session_create` and is deleted exactly
        // once, here, since the session is owned solely by `Resources::sessions`.
        unsafe { ffi::sqlite3session_delete(self.raw) };
    }
}

struct Authorizer {
    function: v8::Global<v8::Function>,
}

impl Resources {
    fn connection(&self, id: u32) -> Result<Rc<Connection>, JsErrorBox> {
        self.connections
            .get(&id)
            .cloned()
            .ok_or_else(|| error("ERR_INVALID_STATE", "database is not open"))
    }

    fn statement(&mut self, id: u32) -> Result<&mut PreparedStatement, JsErrorBox> {
        self.statements
            .get_mut(&id)
            .ok_or_else(|| error("ERR_INVALID_STATE", "statement has been finalized"))
    }

    fn add_connection(&mut self, connection: Connection) -> u32 {
        self.next_connection += 1;
        self.connections
            .insert(self.next_connection, Rc::new(connection));
        self.next_connection
    }

    fn add_statement(&mut self, statement: PreparedStatement) -> u32 {
        self.next_statement += 1;
        self.statements.insert(self.next_statement, statement);
        self.next_statement
    }

    fn add_session(&mut self, session: Session) -> u32 {
        self.next_session += 1;
        self.sessions.insert(self.next_session, session);
        self.next_session
    }

    fn finalize_statement(&mut self, id: u32) -> Result<(), JsErrorBox> {
        match self.statements.get(&id) {
            Some(statement) if statement.busy => Err(error(
                "ERR_INVALID_STATE",
                "statement is already being executed",
            )),
            _ => {
                self.statements.remove(&id);
                Ok(())
            }
        }
    }

    fn close_connection(&mut self, id: u32) -> Result<(), JsErrorBox> {
        let Some(connection) = self.connections.get(&id).cloned() else {
            return Ok(());
        };
        if self
            .statements
            .values()
            .any(|statement| statement.busy && Rc::ptr_eq(&statement.connection, &connection))
        {
            return Err(error(
                "ERR_INVALID_STATE",
                "database cannot be closed while in a callback",
            ));
        }
        // SAFETY: the connection is still registered here; detaching the
        // authorizer before its box is dropped below keeps the callback from
        // outliving the data SQLite points it at.
        unsafe {
            ffi::sqlite3_set_authorizer(connection.handle(), None, ptr::null_mut());
        }
        self.authorizers.remove(&id);
        self.connections.remove(&id);
        self.sessions
            .retain(|_, session| !Rc::ptr_eq(&session.connection, &connection));
        self.statements
            .retain(|_, statement| !Rc::ptr_eq(&statement.connection, &connection));
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "each boolean is one independent `node:sqlite` open option"
    )]
    pub(crate) fn open(
        state: &Rc<RefCell<OpState>>,
        path: &str,
        read_only: bool,
        foreign_keys: bool,
        double_quoted_strings: bool,
        defensive: bool,
        allow_extension: bool,
        timeout: u32,
    ) -> Result<u32, JsErrorBox> {
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
        };
        let connection = Connection::open_with_flags(path, flags).map_err(rusqlite_error)?;
        // SAFETY: the connection was just opened and is owned here.
        unsafe {
            ffi::sqlite3_enable_load_extension(connection.handle(), i32::from(allow_extension));
        }
        connection
            .busy_timeout(Duration::from_millis(timeout.into()))
            .map_err(rusqlite_error)?;
        connection
            .execute_batch(if foreign_keys {
                "PRAGMA foreign_keys = ON"
            } else {
                "PRAGMA foreign_keys = OFF"
            })
            .map_err(rusqlite_error)?;

        // SAFETY: the connection is alive for the rest of the op.
        let handle = unsafe { connection.handle() };
        for (config, active) in [
            (ffi::SQLITE_DBCONFIG_DQS_DML, double_quoted_strings),
            (ffi::SQLITE_DBCONFIG_DQS_DDL, double_quoted_strings),
            (ffi::SQLITE_DBCONFIG_DEFENSIVE, defensive),
        ] {
            let mut current: c_int = 0;
            // SAFETY: `handle` is the live connection and `current` is a writable
            // out-parameter for the previous value.
            let rc = unsafe {
                ffi::sqlite3_db_config(handle, config, c_int::from(active), &raw mut current)
            };
            if rc != ffi::SQLITE_OK {
                return Err(connection_error(&connection));
            }
        }

        let mut op_state = state.borrow_mut();
        Ok(op_state.borrow_mut::<Self>().add_connection(connection))
    }

    pub(crate) fn close(state: &Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state.borrow_mut::<Self>().close_connection(id)
    }

    pub(crate) fn exec(
        scope: &mut v8::PinScope<'_, '_>,
        state: &Rc<RefCell<OpState>>,
        id: u32,
        sql: &str,
    ) -> Result<(), JsErrorBox> {
        let sql = CString::new(sql).map_err(|_| {
            error(
                "ERR_INVALID_ARG_VALUE",
                "The SQL query contains a null byte.",
            )
        })?;
        let connection = { state.borrow().borrow::<Self>().connection(id)? };
        let mut message: *mut c_char = ptr::null_mut();
        let rc = {
            let _installed = callbacks::install(scope, state);
            // SAFETY: the connection and `sql` are alive for the whole call, and
            // `message` is only written by SQLite.
            unsafe {
                ffi::sqlite3_exec(
                    connection.handle(),
                    sql.as_ptr(),
                    None,
                    ptr::null_mut(),
                    &raw mut message,
                )
            }
        };
        let failure = if rc == ffi::SQLITE_OK {
            None
        } else {
            let text = if message.is_null() {
                None
            } else {
                // SAFETY: the null case is handled above; SQLite returns a
                // NUL-terminated message allocated with `sqlite3_malloc`.
                let text = unsafe { CStr::from_ptr(message) }
                    .to_string_lossy()
                    .into_owned();
                // SAFETY: `message` was allocated by `sqlite3_exec` and is freed
                // exactly once here.
                unsafe { ffi::sqlite3_free(message.cast::<c_void>()) };
                Some(text)
            };
            Some(sqlite_error(rc, &text.unwrap_or_else(|| errstr(rc))))
        };
        callbacks::finish(state, failure)
    }

    pub(crate) fn is_autocommit(state: &Rc<RefCell<OpState>>, id: u32) -> Result<bool, JsErrorBox> {
        let connection = { state.borrow().borrow::<Self>().connection(id)? };
        // SAFETY: the connection is registered and alive here.
        Ok(unsafe { ffi::sqlite3_get_autocommit(connection.handle()) } != 0)
    }
}
