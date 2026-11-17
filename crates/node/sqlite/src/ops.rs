use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::time::Duration;

use deno_core::{OpDecl, OpState, op2, v8};
use deno_error::JsErrorBox;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::ffi;

use crate::backup;
use crate::callbacks;
use crate::errors::{connection_error, error, errstr, rusqlite_error, sqlite_error};
use crate::session::{self, Session};
use crate::{authorizer, extras, functions, modules};

pub(crate) const OPS: &[OpDecl] = &[
    op_sqlite_open(),
    op_sqlite_close(),
    op_sqlite_exec(),
    op_sqlite_prepare(),
    op_sqlite_finalize(),
    op_sqlite_parameter_names(),
    op_sqlite_clear_bindings(),
    op_sqlite_bind_null(),
    op_sqlite_bind_integer(),
    op_sqlite_bind_real(),
    op_sqlite_bind_text(),
    op_sqlite_bind_blob(),
    op_sqlite_step(),
    op_sqlite_reset(),
    op_sqlite_columns(),
    op_sqlite_changes(),
    op_sqlite_is_autocommit(),
    op_sqlite_expanded_sql(),
    functions::op_sqlite_create_function(),
    functions::op_sqlite_take_callback_error(),
    session::op_sqlite_session_create(),
    session::op_sqlite_session_changeset(),
    session::op_sqlite_session_close(),
    session::op_sqlite_apply_changeset(),
    backup::op_sqlite_backup(),
    extras::op_sqlite_location(),
    extras::op_sqlite_limit(),
    extras::op_sqlite_set_limit(),
    extras::op_sqlite_serialize(),
    extras::op_sqlite_deserialize(),
    extras::op_sqlite_enable_defensive(),
    extras::op_sqlite_enable_load_extension(),
    extras::op_sqlite_load_extension(),
    extras::op_sqlite_statement_status(),
    extras::op_sqlite_statement_reset_status(),
    authorizer::op_sqlite_set_authorizer(),
    modules::op_sqlite_create_module(),
];

#[derive(Default)]
pub(crate) struct Handles {
    next_connection: u32,
    next_statement: u32,
    next_session: u32,
    connections: HashMap<u32, Rc<Connection>>,
    statements: HashMap<u32, PreparedStatement>,
    pub(crate) sessions: HashMap<u32, Session>,
    pub(crate) authorizers: HashMap<u32, Box<crate::authorizer::Authorizer>>,
}

impl Handles {
    fn add_connection(&mut self, connection: Connection) -> u32 {
        self.next_connection += 1;
        self.connections
            .insert(self.next_connection, Rc::new(connection));
        self.next_connection
    }

    pub(crate) fn add_session(&mut self, session: Session) -> u32 {
        self.next_session += 1;
        self.sessions.insert(self.next_session, session);
        self.next_session
    }
    fn close(&mut self, id: u32) -> Result<(), JsErrorBox> {
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
        if let Some(connection) = self.connections.get(&id) {
            // SAFETY: the connection is still registered here; detaching the
            // authorizer before its box is dropped below keeps the callback from
            // outliving the data SQLite points it at.
            unsafe {
                ffi::sqlite3_set_authorizer(connection.handle(), None, std::ptr::null_mut());
            }
        }
        self.authorizers.remove(&id);
        self.connections.remove(&id);
        self.sessions
            .retain(|_, session| !Rc::ptr_eq(&session.connection, &connection));
        self.statements
            .retain(|_, statement| !Rc::ptr_eq(&statement.connection, &connection));
        Ok(())
    }
}
pub(crate) struct PreparedStatement {
    connection: Rc<Connection>,
    pub(crate) raw: *mut ffi::sqlite3_stmt,
    busy: bool,
}

impl Drop for PreparedStatement {
    fn drop(&mut self) {
        // SAFETY: `raw` comes from `sqlite3_prepare_v3` and is finalized exactly
        // once, since the statement is owned solely by `Handles::statements`.
        unsafe { ffi::sqlite3_finalize(self.raw) };
    }
}

pub(crate) fn connection(handles: &Handles, id: u32) -> Result<Rc<Connection>, JsErrorBox> {
    handles
        .connections
        .get(&id)
        .cloned()
        .ok_or_else(|| error("ERR_INVALID_STATE", "database is not open"))
}
pub(crate) fn connection_of(
    state: &Rc<RefCell<OpState>>,
    id: u32,
) -> Result<Rc<Connection>, JsErrorBox> {
    let op_state = state.borrow();
    connection(op_state.borrow::<Handles>(), id)
}
pub(crate) fn check(rc: c_int, connection: &Connection) -> Result<(), JsErrorBox> {
    if rc == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(connection_error(connection))
    }
}

pub(crate) fn statement(
    handles: &mut Handles,
    id: u32,
) -> Result<&mut PreparedStatement, JsErrorBox> {
    handles
        .statements
        .get_mut(&id)
        .ok_or_else(|| error("ERR_INVALID_STATE", "statement has been finalized"))
}

#[op2(fast)]
fn op_sqlite_open(
    state: Rc<RefCell<OpState>>,
    #[string] path: &str,
    read_only: bool,
    foreign_keys: bool,
    double_quoted_strings: bool,
    defensive: bool,
    allow_extension: bool,
    #[smi] timeout: u32,
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
    Ok(op_state.borrow_mut::<Handles>().add_connection(connection))
}

#[op2(fast)]
fn op_sqlite_close(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    op_state.borrow_mut::<Handles>().close(id)
}

#[op2(nofast, reentrant)]
fn op_sqlite_exec(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] sql: &str,
) -> Result<(), JsErrorBox> {
    let sql = CString::new(sql).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The SQL query contains a null byte.",
        )
    })?;
    let connection = connection_of(&state, id)?;
    let mut message: *mut c_char = ptr::null_mut();
    let rc = {
        let _installed = callbacks::install(scope, &state);
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
    callbacks::finish(&state, failure)
}

#[op2(nofast, reentrant)]
fn op_sqlite_prepare(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] connection_id: u32,
    #[string] sql: &str,
    persistent: bool,
) -> Result<u32, JsErrorBox> {
    let connection = connection_of(&state, connection_id)?;

    let length = c_int::try_from(sql.len())
        .map_err(|_| error("ERR_INVALID_ARG_VALUE", "The SQL query is too large."))?;
    let mut raw: *mut ffi::sqlite3_stmt = ptr::null_mut();
    let flags = if persistent {
        ffi::SQLITE_PREPARE_PERSISTENT
    } else {
        0
    };
    let rc = {
        let _installed = callbacks::install(scope, &state);
        // SAFETY: the connection and `sql` are alive for the whole call; SQLite
        // writes the prepared statement into `raw`, which `PreparedStatement`
        // then owns.
        unsafe {
            ffi::sqlite3_prepare_v3(
                connection.handle(),
                sql.as_ptr().cast::<c_char>(),
                length,
                flags,
                &raw mut raw,
                ptr::null_mut(),
            )
        }
    };
    if rc != ffi::SQLITE_OK {
        return callbacks::finish(&state, Some(connection_error(&connection)))
            .map(|()| unreachable!());
    }
    if raw.is_null() {
        return Err(error(
            "ERR_INVALID_ARG_VALUE",
            "The SQL query contains no statements.",
        ));
    }

    let mut op_state = state.borrow_mut();
    let handles = op_state.borrow_mut::<Handles>();
    handles.next_statement += 1;
    let id = handles.next_statement;
    handles.statements.insert(
        id,
        PreparedStatement {
            connection,
            raw,
            busy: false,
        },
    );
    Ok(id)
}

#[op2(fast)]
fn op_sqlite_finalize(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let handles = op_state.borrow_mut::<Handles>();
    match handles.statements.get(&id) {
        Some(statement) if statement.busy => Err(error(
            "ERR_INVALID_STATE",
            "statement is already being executed",
        )),
        _ => {
            handles.statements.remove(&id);
            Ok(())
        }
    }
}

#[op2]
#[string]
fn op_sqlite_parameter_names(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    Ok(statement.parameter_names())
}

#[op2(fast)]
fn op_sqlite_clear_bindings(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    statement.clear_bindings();
    Ok(())
}

#[op2(fast)]
fn op_sqlite_bind_null(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    statement.bind(index, |raw, index| {
        // SAFETY: `raw` is the statement's own handle and SQLite validates
        // `index` against the parameter count.
        unsafe { ffi::sqlite3_bind_null(raw, index) }
    })
}

#[op2(fast)]
fn op_sqlite_bind_integer(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    #[bigint] value: i64,
) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    statement.bind(index, |raw, index| {
        // SAFETY: `raw` is the statement's own handle and SQLite validates
        // `index` against the parameter count.
        unsafe { ffi::sqlite3_bind_int64(raw, index, value) }
    })
}

#[op2(fast)]
fn op_sqlite_bind_real(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    value: f64,
) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    statement.bind(index, |raw, index| {
        // SAFETY: `raw` is the statement's own handle and SQLite validates
        // `index` against the parameter count.
        unsafe { ffi::sqlite3_bind_double(raw, index, value) }
    })
}

#[op2(fast)]
fn op_sqlite_bind_text(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    #[string] value: &str,
) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    statement.bind(index, |raw, index| {
        // SAFETY: `raw` is the statement's own handle, SQLite validates `index`,
        // and SQLITE_TRANSIENT copies `value` before the call returns.
        unsafe {
            ffi::sqlite3_bind_text64(
                raw,
                index,
                value.as_ptr().cast::<c_char>(),
                value.len() as u64,
                ffi::SQLITE_TRANSIENT(),
                callbacks::UTF8,
            )
        }
    })
}

#[op2(fast)]
fn op_sqlite_bind_blob(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[smi] index: u32,
    #[buffer(copy)] value: Vec<u8>,
) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    statement.bind(index, |raw, index| {
        // SAFETY: `raw` is the statement's own handle, SQLite validates `index`,
        // and SQLITE_TRANSIENT copies `value` before the call returns.
        unsafe {
            ffi::sqlite3_bind_blob64(
                raw,
                index,
                value.as_ptr().cast::<c_void>(),
                value.len() as u64,
                ffi::SQLITE_TRANSIENT(),
            )
        }
    })
}

#[op2(reentrant)]
#[buffer]
fn op_sqlite_step(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<Box<[u8]>, JsErrorBox> {
    let (raw, connection) = {
        let mut op_state = state.borrow_mut();
        let handles = op_state.borrow_mut::<Handles>();
        let statement = statement(handles, id)?;
        if statement.busy {
            return Err(error(
                "ERR_INVALID_STATE",
                "statement is already being executed",
            ));
        }
        statement.busy = true;
        (statement.raw, statement.connection.clone())
    };

    let rc = {
        let _installed = callbacks::install(scope, &state);
        // SAFETY: `raw` is a live prepared statement; the busy flag above keeps it
        // from being finalized while the step runs.
        unsafe { ffi::sqlite3_step(raw) }
    };

    let outcome = {
        let mut op_state = state.borrow_mut();
        let handles = op_state.borrow_mut::<Handles>();
        let Some(statement) = handles.statements.get_mut(&id) else {
            return Err(error("ERR_INVALID_STATE", "statement has been finalized"));
        };
        statement.busy = false;
        match rc {
            ffi::SQLITE_ROW => Ok(statement.encode_row().into_boxed_slice()),
            ffi::SQLITE_DONE => Ok(Box::default()),
            _ => Err(connection_error(&connection)),
        }
    };
    match outcome {
        Ok(row) => callbacks::finish(&state, None).map(|()| row),
        Err(failure) => callbacks::finish(&state, Some(failure)).map(|()| Box::default()),
    }
}

#[op2(fast)]
fn op_sqlite_reset(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<(), JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    // SAFETY: the statement is registered and not finalized while `Handles`
    // borrows it.
    unsafe { ffi::sqlite3_reset(statement.raw) };
    Ok(())
}

#[op2]
#[buffer]
fn op_sqlite_columns(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    Ok(statement.encode_columns().into_boxed_slice())
}

#[op2]
#[buffer]
fn op_sqlite_changes(state: Rc<RefCell<OpState>>, #[smi] id: u32) -> Result<Box<[u8]>, JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    // SAFETY: the connection outlives the statement it prepared.
    let handle = unsafe { statement.connection.handle() };
    let mut out = Vec::with_capacity(16);
    // SAFETY: `handle` is a live connection.
    out.extend_from_slice(&unsafe { ffi::sqlite3_changes64(handle) }.to_le_bytes());
    // SAFETY: same live connection.
    out.extend_from_slice(&unsafe { ffi::sqlite3_last_insert_rowid(handle) }.to_le_bytes());
    Ok(out.into_boxed_slice())
}

#[op2(fast)]
fn op_sqlite_is_autocommit(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<bool, JsErrorBox> {
    let op_state = state.borrow();
    let connection = connection(op_state.borrow::<Handles>(), id)?;
    // SAFETY: the connection is registered and alive.
    Ok(unsafe { ffi::sqlite3_get_autocommit(connection.handle()) } != 0)
}

#[op2]
#[string]
fn op_sqlite_expanded_sql(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let mut op_state = state.borrow_mut();
    let statement = statement(op_state.borrow_mut::<Handles>(), id)?;
    // SAFETY: the statement is live and owned by `Handles`; the returned string is
    // allocated with `sqlite3_malloc` and freed below.
    let pointer = unsafe { ffi::sqlite3_expanded_sql(statement.raw) };
    if pointer.is_null() {
        return Err(error("ERR_INVALID_STATE", "statement has no expanded SQL"));
    }
    // SAFETY: the null case is handled above and SQLite returns a NUL-terminated
    // string.
    let sql = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: `pointer` was allocated by `sqlite3_expanded_sql` and is freed
    // exactly once, after its contents were copied.
    unsafe { ffi::sqlite3_free(pointer.cast::<c_void>()) };
    Ok(sql)
}

impl PreparedStatement {
    fn clear_bindings(&self) {
        // SAFETY: `self.raw` is a live prepared statement and `&self` keeps the
        // statement from being finalized during the call.
        unsafe { ffi::sqlite3_clear_bindings(self.raw) };
    }

    #[expect(
        clippy::cast_possible_wrap,
        reason = "parameter indices arrive as `#[smi]` int32 op arguments"
    )]
    fn bind(
        &self,
        index: u32,
        apply: impl FnOnce(*mut ffi::sqlite3_stmt, c_int) -> c_int,
    ) -> Result<(), JsErrorBox> {
        let rc = apply(self.raw, index as c_int);
        check(rc, &self.connection)
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "SQLite never reports a negative parameter count"
    )]
    fn parameter_count(&self) -> usize {
        // SAFETY: `self.raw` is a live prepared statement.
        let count = unsafe { ffi::sqlite3_bind_parameter_count(self.raw) };
        count as usize
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "parameter indices are bounded by SQLITE_MAX_VARIABLE_NUMBER"
    )]
    fn parameter_names(&self) -> String {
        let count = self.parameter_count();
        let mut names = Vec::with_capacity(count + 1);
        names.push(count.to_string());
        for index in 1..=count {
            // SAFETY: `self.raw` is a live prepared statement and `index` is within
            // its parameter count; the name is owned by SQLite.
            let name = unsafe { ffi::sqlite3_bind_parameter_name(self.raw, index as c_int) };
            names.push(c_text(name).unwrap_or_default());
        }
        names.join("\0")
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "SQLite never reports a negative column count"
    )]
    fn column_count(&self) -> usize {
        // SAFETY: `self.raw` is a live prepared statement.
        let count = unsafe { ffi::sqlite3_column_count(self.raw) };
        count as usize
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "column indices are bounded by SQLITE_MAX_COLUMN"
    )]
    fn column_name(&self, index: usize) -> String {
        // SAFETY: `self.raw` is a live prepared statement and `index` is within its
        // column count; the name is owned by SQLite.
        c_text(unsafe { ffi::sqlite3_column_name(self.raw, index as c_int) }).unwrap_or_default()
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "column indices are bounded by SQLITE_MAX_COLUMN"
    )]
    fn column_metadata(&self, index: usize) -> [Option<String>; 4] {
        let index = index as c_int;
        // SAFETY: `self.raw` is a live prepared statement and `index` is a valid
        // column of it; all four names are owned by SQLite.
        unsafe {
            [
                c_text(ffi::sqlite3_column_origin_name(self.raw, index)),
                c_text(ffi::sqlite3_column_database_name(self.raw, index)),
                c_text(ffi::sqlite3_column_table_name(self.raw, index)),
                c_text(ffi::sqlite3_column_decltype(self.raw, index)),
            ]
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "column counts and indices are bounded by SQLITE_MAX_COLUMN"
    )]
    fn encode_row(&self) -> Vec<u8> {
        let count = self.column_count();
        let mut out = Vec::with_capacity(16);
        push_u32(&mut out, count as u32);
        for index in 0..count {
            let column = index as c_int;
            // SAFETY: `self.raw` is a live prepared statement holding the current
            // row and `column` is one of its columns.
            let kind = unsafe { ffi::sqlite3_column_type(self.raw, column) };
            match kind {
                ffi::SQLITE_NULL => out.push(0),
                ffi::SQLITE_INTEGER => {
                    out.push(1);
                    // SAFETY: same live row and column.
                    push_i64(&mut out, unsafe {
                        ffi::sqlite3_column_int64(self.raw, column)
                    });
                }
                ffi::SQLITE_FLOAT => {
                    out.push(2);
                    // SAFETY: same live row and column.
                    push_f64(&mut out, unsafe {
                        ffi::sqlite3_column_double(self.raw, column)
                    });
                }
                ffi::SQLITE_TEXT => {
                    // SAFETY: same live row and column; the returned text stays
                    // valid until the statement is stepped or finalized.
                    let text = unsafe { ffi::sqlite3_column_text(self.raw, column) };
                    // SAFETY: same live row and column.
                    let length = unsafe { ffi::sqlite3_column_bytes(self.raw, column) };
                    out.push(3);
                    push_bytes(&mut out, callbacks::slice(text.cast::<u8>(), length));
                }
                _ => {
                    // SAFETY: same live row and column; the returned blob stays
                    // valid until the statement is stepped or finalized.
                    let blob = unsafe { ffi::sqlite3_column_blob(self.raw, column) };
                    // SAFETY: same live row and column.
                    let length = unsafe { ffi::sqlite3_column_bytes(self.raw, column) };
                    out.push(4);
                    push_bytes(&mut out, callbacks::slice(blob.cast::<u8>(), length));
                }
            }
        }
        out
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "column counts are bounded by SQLITE_MAX_COLUMN"
    )]
    fn encode_columns(&self) -> Vec<u8> {
        let count = self.column_count();
        let mut out = Vec::new();
        push_u32(&mut out, count as u32);
        for index in 0..count {
            let [origin, database, table, decl_type] = self.column_metadata(index);
            push_optional(&mut out, origin);
            push_optional(&mut out, database);
            push_bytes(&mut out, self.column_name(index).as_bytes());
            push_optional(&mut out, table);
            push_optional(&mut out, decl_type);
        }
        out
    }
}

fn c_text(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null()).then(|| {
        // SAFETY: the null case is excluded by the guard, and SQLite owns the
        // NUL-terminated string for as long as its statement lives.
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    })
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "SQLite text and blob values are shorter than u32::MAX (SQLITE_MAX_LENGTH is 1e9)"
)]
fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn push_optional(out: &mut Vec<u8>, text: Option<String>) {
    match text {
        Some(text) => push_bytes(out, text.as_bytes()),
        None => push_u32(out, u32::MAX),
    }
}
