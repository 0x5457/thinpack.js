use std::cell::RefCell;
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use std::rc::Rc;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use super::Resources;
use crate::callbacks;
use crate::errors::{connection_error, error};

const SCALAR: u32 = 0;

/// Bytes `sqlite3_aggregate_context` reserves per group: the pointer to the
/// boxed [`Group`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "a pointer is never wider than a c_int on a supported target"
)]
const GROUP_SLOT: c_int = std::mem::size_of::<*mut Group>() as c_int;

struct Scalar {
    function: v8::Global<v8::Function>,
    big_int: bool,
}

struct Aggregate {
    start: v8::Global<v8::Value>,
    step: v8::Global<v8::Function>,
    result: Option<v8::Global<v8::Function>>,
    inverse: Option<v8::Global<v8::Function>>,
    big_int: bool,
}

#[derive(Default)]
struct Group {
    accumulator: Option<v8::Global<v8::Value>>,
    failed: bool,
}

unsafe extern "C" fn destroy<T>(data: *mut c_void) {
    // SAFETY: `data` is a `Box<T>` leaked by `Box::into_raw` in this module and
    // SQLite calls this destructor exactly once per registration.
    drop(unsafe { Box::from_raw(data.cast::<T>()) });
}

unsafe extern "C" fn scalar_call(
    context: *mut ffi::sqlite3_context,
    count: c_int,
    args: *mut *mut ffi::sqlite3_value,
) {
    // SAFETY: `context` is the function context of this call, and
    // `sqlite3_user_data` yields the `Scalar` registered for it, which SQLite
    // frees through `destroy::<Scalar>` only after the last call.
    let scalar = unsafe { &*(ffi::sqlite3_user_data(context) as *const Scalar) };
    #[expect(
        clippy::cast_sign_loss,
        reason = "SQLite never passes a negative argument count"
    )]
    let count = count as usize;
    let outcome = callbacks::with_frame(|scope, state| -> Result<(), ()> {
        let mut arguments = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: SQLite passes exactly `count` argument pointers.
            let value = unsafe { *args.add(index) };
            arguments.push(callbacks::to_js(scope, state, value, scalar.big_int).ok_or(())?);
        }
        let function = v8::Local::new(scope, &scalar.function);
        let returned = callbacks::invoke(
            scope,
            state,
            function,
            v8::undefined(scope).into(),
            &arguments,
        )
        .ok_or(())?;
        callbacks::to_sqlite(scope, context, returned);
        Ok(())
    });
    match outcome {
        Some(Ok(())) => {}
        Some(Err(())) => callbacks::result_error(context, "the JavaScript function failed"),
        // Unreachable through the public entry points — they all run inside a
        // statement, which installs the callback frame `with_frame` needs. Kept
        // as the loud failure if a future entry point forgets it.
        None => callbacks::result_error(
            context,
            "the JavaScript function cannot be invoked outside a statement",
        ),
    }
}

unsafe fn group(context: *mut ffi::sqlite3_context) -> Option<&'static mut Group> {
    // SAFETY: SQLite hands out a zero-initialized `GROUP_SLOT`-byte slot per
    // aggregate group that lives until the final call.
    let slot = unsafe { ffi::sqlite3_aggregate_context(context, GROUP_SLOT) };
    if slot.is_null() {
        return None;
    }
    let slot = slot.cast::<*mut Group>();
    // SAFETY: the slot is zeroed on first use and only written through this
    // module, so reading the stored pointer is sound.
    if unsafe { (*slot).is_null() } {
        // SAFETY: the slot belongs to the current group and is not yet populated.
        unsafe { *slot = Box::into_raw(Box::new(Group::default())) };
    }
    // SAFETY: the box leaked just above (or by an earlier step) is still owned by
    // the slot, which `take_group` clears only at the final call.
    let group = unsafe { &mut **slot };
    Some(group)
}

unsafe fn peek_group(context: *mut ffi::sqlite3_context) -> Option<&'static mut Group> {
    // SAFETY: same aggregate slot as `group`, but read before it was ever written.
    let slot = unsafe { ffi::sqlite3_aggregate_context(context, GROUP_SLOT) };
    if slot.is_null() {
        return None;
    }
    // SAFETY: `slot` holds a single pointer written by `group`.
    let pointer = unsafe { *slot.cast::<*mut Group>() };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: a non-null pointer in the slot is a live box owned by the slot.
    let group = unsafe { &mut *pointer };
    Some(group)
}

unsafe fn take_group(context: *mut ffi::sqlite3_context) -> Option<Box<Group>> {
    // SAFETY: same aggregate slot as `group`; the final call runs once per group.
    let slot = unsafe { ffi::sqlite3_aggregate_context(context, GROUP_SLOT) };
    if slot.is_null() {
        return None;
    }
    let slot = slot.cast::<*mut Group>();
    // SAFETY: `slot` holds a single pointer written by `group`.
    let pointer = unsafe { std::mem::replace(&mut *slot, ptr::null_mut()) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the pointer was cleared from the slot just above, so this takes the
    // only remaining owner of the box.
    let group = unsafe { Box::from_raw(pointer) };
    Some(group)
}

fn start_accumulator(
    scope: &mut v8::PinScope<'_, '_>,
    state: &Rc<RefCell<OpState>>,
    start: &v8::Global<v8::Value>,
) -> Option<v8::Global<v8::Value>> {
    let value = v8::Local::new(scope, start);
    if let Ok(function) = value.try_cast::<v8::Function>() {
        let returned = callbacks::invoke(scope, state, function, v8::undefined(scope).into(), &[])?;
        return Some(v8::Global::new(scope, returned));
    }
    Some(v8::Global::new(scope, value))
}

unsafe fn accumulate(
    context: *mut ffi::sqlite3_context,
    count: c_int,
    args: *mut *mut ffi::sqlite3_value,
    inverse: bool,
) -> Result<(), ()> {
    // SAFETY: `context` is the function context of this step, and
    // `sqlite3_user_data` yields the `Aggregate` registered for it, which SQLite
    // frees through `destroy::<Aggregate>` only after the last call.
    let aggregate = unsafe { &*(ffi::sqlite3_user_data(context) as *const Aggregate) };
    // SAFETY: the step callback always runs with an aggregate slot available.
    let group = unsafe { group(context) }.ok_or(())?;
    if group.failed {
        return Ok(());
    }
    let function = match (inverse, &aggregate.inverse) {
        (true, Some(inverse)) => inverse,
        (true, None) => return Ok(()),
        (false, _) => &aggregate.step,
    };
    #[expect(
        clippy::cast_sign_loss,
        reason = "SQLite never passes a negative argument count"
    )]
    let count = count as usize;
    let outcome = callbacks::with_frame(|scope, state| -> Result<(), ()> {
        if group.accumulator.is_none() {
            group.accumulator = Some(start_accumulator(scope, state, &aggregate.start).ok_or(())?);
        }
        let accumulator = v8::Local::new(scope, group.accumulator.as_ref().unwrap());
        let mut arguments = Vec::with_capacity(count + 1);
        arguments.push(accumulator);
        for index in 0..count {
            // SAFETY: SQLite passes exactly `count` argument pointers.
            let value = unsafe { *args.add(index) };
            arguments.push(callbacks::to_js(scope, state, value, aggregate.big_int).ok_or(())?);
        }
        let function = v8::Local::new(scope, function);
        let returned = callbacks::invoke(
            scope,
            state,
            function,
            v8::undefined(scope).into(),
            &arguments,
        )
        .ok_or(())?;
        group.accumulator = Some(v8::Global::new(scope, returned));
        Ok(())
    });
    if outcome != Some(Ok(())) {
        group.failed = true;
        callbacks::result_error(context, "the JavaScript aggregate function failed");
        return Err(());
    }
    Ok(())
}

fn emit(
    context: *mut ffi::sqlite3_context,
    aggregate: &Aggregate,
    group: Option<&mut Group>,
    consume: bool,
) {
    let outcome = callbacks::with_frame(|scope, state| -> Result<(), ()> {
        let accumulator = if let Some(accumulator) =
            group.as_ref().and_then(|group| group.accumulator.as_ref())
        {
            v8::Local::new(scope, accumulator)
        } else {
            let initial = start_accumulator(scope, state, &aggregate.start).ok_or(())?;
            v8::Local::new(scope, initial)
        };
        let value = match &aggregate.result {
            Some(result) => {
                let result = v8::Local::new(scope, result);
                callbacks::invoke(
                    scope,
                    state,
                    result,
                    v8::undefined(scope).into(),
                    &[accumulator],
                )
                .ok_or(())?
            }
            None => accumulator,
        };
        callbacks::to_sqlite(scope, context, value);
        Ok(())
    });
    if outcome != Some(Ok(())) {
        callbacks::result_error(context, "the JavaScript aggregate function failed");
    }
    if consume && let Some(group) = group {
        group.accumulator = None;
    }
}

unsafe extern "C" fn aggregate_step(
    context: *mut ffi::sqlite3_context,
    count: c_int,
    args: *mut *mut ffi::sqlite3_value,
) {
    // SAFETY: forwarded from the step callback: `context`, `count` and `args` come
    // from SQLite for this call.
    let _ = unsafe { accumulate(context, count, args, false) };
}

unsafe extern "C" fn aggregate_inverse(
    context: *mut ffi::sqlite3_context,
    count: c_int,
    args: *mut *mut ffi::sqlite3_value,
) {
    // SAFETY: forwarded from the inverse callback: `context`, `count` and `args`
    // come from SQLite for this call.
    let _ = unsafe { accumulate(context, count, args, true) };
}

unsafe extern "C" fn aggregate_final(context: *mut ffi::sqlite3_context) {
    // SAFETY: same registration contract as `accumulate`; the final callback runs
    // once per group before SQLite destroys the registration.
    let aggregate = unsafe { &*(ffi::sqlite3_user_data(context) as *const Aggregate) };
    // SAFETY: the final callback owns the group state and takes it exactly once.
    let group = unsafe { take_group(context) };
    let mut group = group;
    let failed = group.as_ref().is_some_and(|group| group.failed);
    if !failed {
        emit(context, aggregate, group.as_deref_mut(), true);
    }
    drop(group);
}

unsafe extern "C" fn aggregate_value(context: *mut ffi::sqlite3_context) {
    // SAFETY: same registration contract as `accumulate`.
    let aggregate = unsafe { &*(ffi::sqlite3_user_data(context) as *const Aggregate) };
    // SAFETY: the value callback only reads the group state; the final callback
    // frees it afterwards.
    let group = unsafe { peek_group(context) };
    let failed = group.as_ref().is_some_and(|group| group.failed);
    if !failed {
        emit(context, aggregate, group, false);
    }
}

const fn flags_of(deterministic: bool, direct_only: bool) -> c_int {
    let mut flags = ffi::SQLITE_UTF8;
    if deterministic {
        flags |= ffi::SQLITE_DETERMINISTIC;
    }
    if direct_only {
        flags |= ffi::SQLITE_DIRECTONLY;
    }
    flags
}

#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "narrowing the scope here would move the lint onto the `#[op2]` signature that calls it"
)]
fn function_of<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    values: v8::Local<'s, v8::Array>,
    index: u32,
) -> Option<v8::Global<v8::Function>> {
    let value = values.get_index(scope, index)?;
    let function = value.try_cast::<v8::Function>().ok()?;
    Some(v8::Global::new(scope, function))
}

impl Resources {
    #[expect(
        clippy::too_many_arguments,
        reason = "the `node:sqlite` function registration arguments are positional"
    )]
    pub(crate) fn create_function<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        state: &Rc<RefCell<OpState>>,
        database: u32,
        name: &str,
        arguments: i32,
        kind: u32,
        deterministic: bool,
        direct_only: bool,
        big_int: bool,
        values: v8::Local<'s, v8::Array>,
    ) -> Result<(), JsErrorBox> {
        let name = CString::new(name).map_err(|_| {
            error(
                "ERR_INVALID_ARG_VALUE",
                "The function name contains a null byte.",
            )
        })?;
        let connection = { state.borrow().borrow::<Self>().connection(database)? };
        // SAFETY: the connection is alive for the whole op.
        let handle = unsafe { connection.handle() };
        let flags = flags_of(deterministic, direct_only);

        let registered = if kind == SCALAR {
            let Some(function) = function_of(scope, values, 0) else {
                return Err(error(
                    "ERR_INVALID_ARG_TYPE",
                    "The function must be callable.",
                ));
            };
            let state = Box::new(Scalar { function, big_int });
            let pointer = Box::into_raw(state).cast::<c_void>();
            // SAFETY: `pointer` is a fresh box owned by the registration on success
            // and by the failure branch below otherwise; `destroy::<Scalar>` runs
            // exactly once in either case.
            let rc = unsafe {
                ffi::sqlite3_create_function_v2(
                    handle,
                    name.as_ptr(),
                    arguments,
                    flags,
                    pointer,
                    Some(scalar_call),
                    None,
                    None,
                    Some(destroy::<Scalar>),
                )
            };
            if rc != ffi::SQLITE_OK {
                // SAFETY: SQLite rejected the registration, so it never took
                // ownership of the box.
                unsafe { destroy::<Scalar>(pointer) };
            }
            rc
        } else {
            let Some(start) = values.get_index(scope, 0) else {
                return Err(error(
                    "ERR_INVALID_ARG_TYPE",
                    "The aggregate has no start value.",
                ));
            };
            let Some(step) = function_of(scope, values, 1) else {
                return Err(error(
                    "ERR_INVALID_ARG_TYPE",
                    "The aggregate step must be callable.",
                ));
            };
            let result = function_of(scope, values, 2);
            let inverse = function_of(scope, values, 3);
            let start = v8::Global::new(scope, start);
            let window = inverse.is_some();
            let state = Box::new(Aggregate {
                start,
                step,
                result,
                inverse,
                big_int,
            });
            let pointer = Box::into_raw(state).cast::<c_void>();
            let rc = if window {
                // SAFETY: `pointer` is a fresh box owned by the registration on
                // success and by the failure branch below otherwise.
                unsafe {
                    ffi::sqlite3_create_window_function(
                        handle,
                        name.as_ptr(),
                        arguments,
                        flags,
                        pointer,
                        Some(aggregate_step),
                        Some(aggregate_final),
                        Some(aggregate_value),
                        Some(aggregate_inverse),
                        Some(destroy::<Aggregate>),
                    )
                }
            } else {
                // SAFETY: same ownership contract as the window registration.
                unsafe {
                    ffi::sqlite3_create_function_v2(
                        handle,
                        name.as_ptr(),
                        arguments,
                        flags,
                        pointer,
                        None,
                        Some(aggregate_step),
                        Some(aggregate_final),
                        Some(destroy::<Aggregate>),
                    )
                }
            };
            if rc != ffi::SQLITE_OK {
                // SAFETY: SQLite rejected the registration, so it never took
                // ownership of the box.
                unsafe { destroy::<Aggregate>(pointer) };
            }
            rc
        };

        if registered == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(connection_error(&connection))
        }
    }
}
