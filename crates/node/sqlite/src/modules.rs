use std::cell::RefCell;
use std::ffi::{CString, c_char, c_int, c_void};
use std::rc::Rc;

use deno_core::{OpState, op2, v8};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use crate::callbacks;
use crate::errors::{connection_error, error};
use crate::ops::connection_of;

struct Registration {
    module: Box<ffi::sqlite3_module>,
    rows: v8::Global<v8::Function>,
    declaration: CString,
}

#[repr(C)]
struct VTab {
    base: ffi::sqlite3_vtab,
    registration: *const Registration,
}

#[repr(C)]
struct Cursor {
    base: ffi::sqlite3_vtab_cursor,
    rows: Option<v8::Global<v8::Array>>,
    index: i64,
    count: i64,
}

unsafe extern "C" fn x_connect(
    db: *mut ffi::sqlite3,
    aux: *mut c_void,
    _argc: c_int,
    _argv: *const *const c_char,
    out: *mut *mut ffi::sqlite3_vtab,
    _error: *mut *mut c_char,
) -> c_int {
    let registration = aux.cast::<Registration>();
    // SAFETY: `aux` is the `Registration` pointer handed to
    // `sqlite3_create_module_v2`, which outlives every virtual table of the module.
    let schema = unsafe { &*registration }.declaration.as_ptr();
    // SAFETY: `db` is the connection SQLite is creating the table on, and `schema`
    // is a NUL-terminated declaration owned by the registration.
    let rc = unsafe { ffi::sqlite3_declare_vtab(db, schema) };
    if rc != ffi::SQLITE_OK {
        return rc;
    }
    let vtab = Box::new(VTab {
        // SAFETY: the all-zero `sqlite3_vtab` is SQLite's documented initial state
        // for a new virtual table; every field is a pointer or an integer.
        base: unsafe { std::mem::zeroed() },
        registration,
    });
    // SAFETY: `out` is the out-parameter SQLite passed for the new virtual table,
    // and the box is released by `x_disconnect`.
    unsafe { *out = Box::into_raw(vtab).cast::<ffi::sqlite3_vtab>() };
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_create(
    db: *mut ffi::sqlite3,
    aux: *mut c_void,
    arg_count: c_int,
    arg_values: *const *const c_char,
    out: *mut *mut ffi::sqlite3_vtab,
    error: *mut *mut c_char,
) -> c_int {
    // SAFETY: `x_create` has the same contract as `x_connect`, which it forwards
    // to unchanged.
    unsafe { x_connect(db, aux, arg_count, arg_values, out, error) }
}

unsafe extern "C" fn x_best_index(
    _vtab: *mut ffi::sqlite3_vtab,
    index: *mut ffi::sqlite3_index_info,
) -> c_int {
    // SAFETY: `index` is the planner-provided info struct for this call only.
    let info = unsafe { &mut *index };
    info.estimatedCost = 1.0;
    info.estimatedRows = 100;
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_disconnect(vtab: *mut ffi::sqlite3_vtab) -> c_int {
    // SAFETY: `vtab` was allocated by `x_connect` and SQLite calls this once per
    // virtual table.
    drop(unsafe { Box::from_raw(vtab.cast::<VTab>()) });
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_destroy(vtab: *mut ffi::sqlite3_vtab) -> c_int {
    // SAFETY: same ownership contract as `x_disconnect`, which frees the box.
    unsafe { x_disconnect(vtab) }
}

unsafe extern "C" fn x_open(
    vtab: *mut ffi::sqlite3_vtab,
    out: *mut *mut ffi::sqlite3_vtab_cursor,
) -> c_int {
    let cursor = Box::new(Cursor {
        base: ffi::sqlite3_vtab_cursor { pVtab: vtab },
        rows: None,
        index: 0,
        count: 0,
    });
    // SAFETY: `out` is the out-parameter SQLite passed for the new cursor, and the
    // box is released by `x_close`.
    unsafe { *out = Box::into_raw(cursor).cast::<ffi::sqlite3_vtab_cursor>() };
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_close(pointer: *mut ffi::sqlite3_vtab_cursor) -> c_int {
    // SAFETY: `pointer` was allocated by `x_open` and SQLite calls this once per
    // cursor.
    drop(unsafe { Box::from_raw(pointer.cast::<Cursor>()) });
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_filter(
    pointer: *mut ffi::sqlite3_vtab_cursor,
    _idx_num: c_int,
    _idx_str: *const c_char,
    _argc: c_int,
    _argv: *mut *mut ffi::sqlite3_value,
) -> c_int {
    // SAFETY: `pointer` is the live cursor SQLite opened for this scan.
    let cursor = unsafe { &mut *pointer.cast::<Cursor>() };
    // SAFETY: `cursor.base.pVtab` points at the `VTab` whose `registration` field
    // owns the `rows` callback for as long as the table exists.
    let registration = unsafe { &*(*cursor.base.pVtab.cast::<VTab>()).registration };
    let outcome = callbacks::with_frame(|scope, state| {
        let function = v8::Local::new(scope, &registration.rows);
        let returned = callbacks::invoke(scope, state, function, v8::undefined(scope).into(), &[])?;
        let array = returned.try_cast::<v8::Array>().ok()?;
        cursor.count = i64::from(array.length());
        cursor.index = 0;
        cursor.rows = Some(v8::Global::new(scope, array));
        Some(())
    });
    if outcome == Some(Some(())) {
        ffi::SQLITE_OK
    } else {
        ffi::SQLITE_ERROR
    }
}

unsafe extern "C" fn x_next(pointer: *mut ffi::sqlite3_vtab_cursor) -> c_int {
    // SAFETY: `pointer` is the live cursor SQLite is stepping.
    unsafe { (*pointer.cast::<Cursor>()).index += 1 };
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_eof(pointer: *mut ffi::sqlite3_vtab_cursor) -> c_int {
    // SAFETY: `pointer` is the live cursor SQLite is stepping.
    let cursor = unsafe { &*pointer.cast::<Cursor>() };
    i32::from(cursor.index >= cursor.count)
}

unsafe extern "C" fn x_column(
    pointer: *mut ffi::sqlite3_vtab_cursor,
    context: *mut ffi::sqlite3_context,
    column: c_int,
) -> c_int {
    // SAFETY: `pointer` is the live cursor SQLite is reading from.
    let cursor = unsafe { &*pointer.cast::<Cursor>() };
    let outcome = callbacks::with_frame(|scope, _state| {
        let rows = v8::Local::new(scope, cursor.rows.as_ref()?);
        let row = rows.get_index(scope, u32::try_from(cursor.index).ok()?)?;
        let row = row.try_cast::<v8::Array>().ok()?;
        let value = row
            .get_index(scope, u32::try_from(column).ok()?)
            .unwrap_or_else(|| v8::null(scope).into());
        callbacks::to_sqlite(scope, context, value);
        Some(())
    });
    if outcome == Some(Some(())) {
        ffi::SQLITE_OK
    } else {
        ffi::SQLITE_ERROR
    }
}

unsafe extern "C" fn x_rowid(
    pointer: *mut ffi::sqlite3_vtab_cursor,
    rowid: *mut ffi::sqlite3_int64,
) -> c_int {
    // SAFETY: `pointer` is the live cursor SQLite is stepping and `rowid` is the
    // out-parameter for this call.
    unsafe { *rowid = (*pointer.cast::<Cursor>()).index + 1 };
    ffi::SQLITE_OK
}

const unsafe extern "C" fn x_update(
    _vtab: *mut ffi::sqlite3_vtab,
    _argc: c_int,
    _argv: *mut *mut ffi::sqlite3_value,
    _rowid: *mut ffi::sqlite3_int64,
) -> c_int {
    ffi::SQLITE_READONLY
}

const unsafe extern "C" fn x_rename(_vtab: *mut ffi::sqlite3_vtab, _name: *const c_char) -> c_int {
    ffi::SQLITE_OK
}

fn module_definition() -> ffi::sqlite3_module {
    // SAFETY: the all-zero `sqlite3_module` is SQLite's documented "no callbacks"
    // state; every field is a function pointer or an integer, for which zero is
    // the null representation.
    let mut module: ffi::sqlite3_module = unsafe { std::mem::zeroed() };
    module.iVersion = 1;
    module.xCreate = Some(x_create);
    module.xConnect = Some(x_connect);
    module.xBestIndex = Some(x_best_index);
    module.xDisconnect = Some(x_disconnect);
    module.xDestroy = Some(x_destroy);
    module.xOpen = Some(x_open);
    module.xClose = Some(x_close);
    module.xFilter = Some(x_filter);
    module.xNext = Some(x_next);
    module.xEof = Some(x_eof);
    module.xColumn = Some(x_column);
    module.xRowid = Some(x_rowid);
    module.xUpdate = Some(x_update);
    module.xRename = Some(x_rename);
    module
}

unsafe extern "C" fn destroy_registration(data: *mut c_void) {
    // SAFETY: `data` is the `Registration` leaked by `op_sqlite_create_module` and
    // SQLite calls this destructor exactly once per module registration.
    drop(unsafe { Box::from_raw(data.cast::<Registration>()) });
}

#[op2(nofast, reentrant)]
pub(crate) fn op_sqlite_create_module(
    scope: &mut v8::PinScope<'_, '_>,
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] name: &str,
    #[string] declaration: &str,
    rows: v8::Local<'_, v8::Value>,
) -> Result<(), JsErrorBox> {
    let rows = rows.try_cast::<v8::Function>().map_err(|_| {
        error(
            "ERR_INVALID_ARG_TYPE",
            "The \"options.rows\" argument must be a function.",
        )
    })?;
    let connection = connection_of(&state, id)?;
    let name = CString::new(name).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The module name contains a null byte.",
        )
    })?;
    let declaration = CString::new(declaration).map_err(|_| {
        error(
            "ERR_INVALID_ARG_VALUE",
            "The module declaration contains a null byte.",
        )
    })?;
    let registration = Box::new(Registration {
        module: Box::new(module_definition()),
        rows: v8::Global::new(scope, rows),
        declaration,
    });
    let pointer = Box::into_raw(registration);
    // SAFETY: `pointer` is the box leaked just above and is still alive here.
    let module_pointer = unsafe { &(*pointer).module };
    // SAFETY: `pointer` is handed to SQLite, which calls `destroy_registration`
    // exactly once; the failure branch below frees it instead.
    let rc = unsafe {
        ffi::sqlite3_create_module_v2(
            connection.handle(),
            name.as_ptr(),
            &raw const **module_pointer,
            pointer.cast::<c_void>(),
            Some(destroy_registration),
        )
    };
    if rc != ffi::SQLITE_OK {
        // SAFETY: SQLite rejected the registration, so it never took ownership of
        // the box.
        unsafe { drop(Box::from_raw(pointer)) };
        return Err(connection_error(&connection));
    }
    Ok(())
}
