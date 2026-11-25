use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use super::{PreparedStatement, Resources};
use crate::callbacks;
use crate::errors::{check, connection_error, error};

impl Resources {
    pub(crate) fn prepare(
        scope: &mut v8::PinScope<'_, '_>,
        state: &Rc<RefCell<OpState>>,
        connection_id: u32,
        sql: &str,
        persistent: bool,
    ) -> Result<u32, JsErrorBox> {
        let connection = { state.borrow().borrow::<Self>().connection(connection_id)? };

        let length = c_int::try_from(sql.len())
            .map_err(|_| error("ERR_INVALID_ARG_VALUE", "The SQL query is too large."))?;
        let mut raw: *mut ffi::sqlite3_stmt = ptr::null_mut();
        let flags = if persistent {
            ffi::SQLITE_PREPARE_PERSISTENT
        } else {
            0
        };
        let rc = {
            let _installed = callbacks::install(scope, state);
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
            return callbacks::finish(state, Some(connection_error(&connection)))
                .map(|()| unreachable!());
        }
        if raw.is_null() {
            return Err(error(
                "ERR_INVALID_ARG_VALUE",
                "The SQL query contains no statements.",
            ));
        }

        let mut op_state = state.borrow_mut();
        Ok(op_state
            .borrow_mut::<Self>()
            .add_statement(PreparedStatement {
                connection,
                raw,
                busy: false,
            }))
    }

    pub(crate) fn finalize(state: &Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state.borrow_mut::<Self>().finalize_statement(id)
    }

    pub(crate) fn parameter_names(
        state: &Rc<RefCell<OpState>>,
        id: u32,
    ) -> Result<String, JsErrorBox> {
        let mut op_state = state.borrow_mut();
        Ok(op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .parameter_names())
    }

    pub(crate) fn clear_bindings(state: &Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .clear_bindings();
        Ok(())
    }

    pub(crate) fn bind_null(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        index: u32,
    ) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .bind_null(index)
    }

    pub(crate) fn bind_integer(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        index: u32,
        value: i64,
    ) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .bind_integer(index, value)
    }

    pub(crate) fn bind_real(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        index: u32,
        value: f64,
    ) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .bind_real(index, value)
    }

    pub(crate) fn bind_text(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        index: u32,
        value: &str,
    ) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .bind_text(index, value)
    }

    pub(crate) fn bind_blob(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        index: u32,
        value: &[u8],
    ) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .bind_blob(index, value)
    }

    pub(crate) fn step(
        scope: &mut v8::PinScope<'_, '_>,
        state: &Rc<RefCell<OpState>>,
        id: u32,
    ) -> Result<Box<[u8]>, JsErrorBox> {
        let (raw, connection) = {
            let mut op_state = state.borrow_mut();
            let statement = op_state.borrow_mut::<Self>().statement(id)?;
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
            let _installed = callbacks::install(scope, state);
            // SAFETY: `raw` is a live prepared statement; the busy flag above keeps
            // it from being finalized while the step runs.
            unsafe { ffi::sqlite3_step(raw) }
        };

        let outcome = {
            let mut op_state = state.borrow_mut();
            let statement = op_state.borrow_mut::<Self>().statement(id)?;
            statement.busy = false;
            match rc {
                ffi::SQLITE_ROW => Ok(statement.encode_row().into_boxed_slice()),
                ffi::SQLITE_DONE => Ok(Box::default()),
                _ => Err(connection_error(&connection)),
            }
        };
        match outcome {
            Ok(row) => callbacks::finish(state, None).map(|()| row),
            Err(failure) => callbacks::finish(state, Some(failure)).map(|()| Box::default()),
        }
    }

    pub(crate) fn reset(state: &Rc<RefCell<OpState>>, id: u32) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        let statement = op_state.borrow_mut::<Self>().statement(id)?;
        // SAFETY: the statement is registered and not finalized while `Resources`
        // borrows it.
        unsafe { ffi::sqlite3_reset(statement.raw) };
        Ok(())
    }

    pub(crate) fn columns(state: &Rc<RefCell<OpState>>, id: u32) -> Result<Box<[u8]>, JsErrorBox> {
        let mut op_state = state.borrow_mut();
        Ok(op_state
            .borrow_mut::<Self>()
            .statement(id)?
            .encode_columns()
            .into_boxed_slice())
    }

    pub(crate) fn changes(state: &Rc<RefCell<OpState>>, id: u32) -> Result<Box<[u8]>, JsErrorBox> {
        let mut op_state = state.borrow_mut();
        let statement = op_state.borrow_mut::<Self>().statement(id)?;
        // SAFETY: the connection outlives the statement it prepared.
        let handle = unsafe { statement.connection.handle() };
        let mut out = Vec::with_capacity(16);
        // SAFETY: `handle` is a live connection.
        out.extend_from_slice(&unsafe { ffi::sqlite3_changes64(handle) }.to_le_bytes());
        // SAFETY: same live connection.
        out.extend_from_slice(&unsafe { ffi::sqlite3_last_insert_rowid(handle) }.to_le_bytes());
        Ok(out.into_boxed_slice())
    }

    pub(crate) fn expanded_sql(
        state: &Rc<RefCell<OpState>>,
        id: u32,
    ) -> Result<String, JsErrorBox> {
        let mut op_state = state.borrow_mut();
        let statement = op_state.borrow_mut::<Self>().statement(id)?;
        // SAFETY: the statement is live and owned by `Resources`; the returned
        // string is allocated with `sqlite3_malloc` and freed below.
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

    pub(crate) fn statement_status(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        counter: i32,
    ) -> Result<i32, JsErrorBox> {
        let mut op_state = state.borrow_mut();
        let statement = op_state.borrow_mut::<Self>().statement(id)?;
        // SAFETY: the statement is registered and only finalized by `Resources`;
        // reset is 0, so this only reads the counter.
        Ok(unsafe { ffi::sqlite3_stmt_status(statement.raw, counter, 0) })
    }

    pub(crate) fn statement_reset_status(
        state: &Rc<RefCell<OpState>>,
        id: u32,
        counter: i32,
        all: bool,
    ) -> Result<(), JsErrorBox> {
        let mut op_state = state.borrow_mut();
        let statement = op_state.borrow_mut::<Self>().statement(id)?;
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

    fn bind_null(&self, index: u32) -> Result<(), JsErrorBox> {
        self.bind(index, |raw, index| {
            // SAFETY: `raw` is the statement's own handle and SQLite validates
            // `index` against the parameter count.
            unsafe { ffi::sqlite3_bind_null(raw, index) }
        })
    }

    fn bind_integer(&self, index: u32, value: i64) -> Result<(), JsErrorBox> {
        self.bind(index, |raw, index| {
            // SAFETY: `raw` is the statement's own handle and SQLite validates
            // `index` against the parameter count.
            unsafe { ffi::sqlite3_bind_int64(raw, index, value) }
        })
    }

    fn bind_real(&self, index: u32, value: f64) -> Result<(), JsErrorBox> {
        self.bind(index, |raw, index| {
            // SAFETY: `raw` is the statement's own handle and SQLite validates
            // `index` against the parameter count.
            unsafe { ffi::sqlite3_bind_double(raw, index, value) }
        })
    }

    fn bind_text(&self, index: u32, value: &str) -> Result<(), JsErrorBox> {
        self.bind(index, |raw, index| {
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

    fn bind_blob(&self, index: u32, value: &[u8]) -> Result<(), JsErrorBox> {
        self.bind(index, |raw, index| {
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
