use std::cell::RefCell;
use std::ffi::{CString, c_char, c_int, c_uchar};
use std::rc::Rc;

use deno_core::{OpState, v8};
use deno_error::JsErrorBox;
use rusqlite::ffi;

use crate::errors::error;

pub(crate) const CALLBACK_ERROR: &str = "ERR_SQLITE_CALLBACK";

/// The `enc` argument of `sqlite3_result_text64`/`sqlite3_bind_text64` is a
/// single byte.
#[expect(
    clippy::cast_possible_truncation,
    reason = "SQLITE_UTF8 is 1 and fits in the single-byte encoding argument"
)]
pub(crate) const UTF8: c_uchar = ffi::SQLITE_UTF8 as u8;

const OUT_OF_RANGE: &str = "ERR_OUT_OF_RANGE";
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Default)]
pub(crate) struct Pending(Option<v8::Global<v8::Value>>);

type ScopePointer = *mut v8::PinScope<'static, 'static>;

#[derive(Clone)]
struct Frame {
    scope: ScopePointer,
    state: Rc<RefCell<OpState>>,
}

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

pub(crate) struct Installed {
    previous: Option<Frame>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        FRAME.with(|slot| slot.borrow_mut().clone_from(&self.previous));
    }
}

pub(crate) fn install(scope: &mut v8::PinScope<'_, '_>, state: &Rc<RefCell<OpState>>) -> Installed {
    let scope: *mut v8::PinScope<'_, '_> = scope;
    let frame = Frame {
        scope: scope.cast(),
        state: state.clone(),
    };
    let previous = FRAME.with(|slot| slot.borrow_mut().replace(frame));
    Installed { previous }
}

pub(crate) fn with_frame<T>(
    run: impl FnOnce(&mut v8::PinScope<'_, '_>, &Rc<RefCell<OpState>>) -> T,
) -> Option<T> {
    let frame = FRAME.with(|slot| slot.borrow().clone())?;
    // SAFETY: the frame is only installed for the duration of the `Installed`
    // guard held by the op that installed it, and calls cannot outlive it.
    let scope: &mut v8::PinScope<'_, '_> = unsafe { &mut *frame.scope };
    Some(run(scope, &frame.state))
}

pub(crate) fn record(
    scope: &mut v8::PinScope<'_, '_>,
    state: &Rc<RefCell<OpState>>,
    exception: v8::Local<'_, v8::Value>,
) {
    let pending = v8::Global::new(scope, exception);
    state.borrow_mut().borrow_mut::<Pending>().0 = Some(pending);
}

pub(crate) fn invoke<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &Rc<RefCell<OpState>>,
    function: v8::Local<'s, v8::Function>,
    receiver: v8::Local<'s, v8::Value>,
    arguments: &[v8::Local<'s, v8::Value>],
) -> Option<v8::Local<'s, v8::Value>> {
    v8::tc_scope!(let tc_scope, scope);
    let returned = function.call(tc_scope, receiver, arguments);
    if tc_scope.has_caught() {
        if let Some(exception) = tc_scope.exception() {
            record(tc_scope, state, exception);
        }
        return None;
    }
    returned
}

pub(crate) fn take_pending<'s>(
    scope: &v8::PinScope<'s, '_>,
    state: &Rc<RefCell<OpState>>,
) -> Option<v8::Local<'s, v8::Value>> {
    let pending = state.borrow_mut().borrow_mut::<Pending>().0.take()?;
    Some(v8::Local::new(scope, pending))
}

pub(crate) fn finish(
    state: &Rc<RefCell<OpState>>,
    failure: Option<JsErrorBox>,
) -> Result<(), JsErrorBox> {
    if state.borrow().borrow::<Pending>().0.is_some() {
        return Err(error(CALLBACK_ERROR, "a JavaScript callback threw"));
    }
    failure.map_or(Ok(()), Err)
}

pub(crate) fn to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &Rc<RefCell<OpState>>,
    value: *mut ffi::sqlite3_value,
    big_int: bool,
) -> Option<v8::Local<'s, v8::Value>> {
    // SAFETY: `value` is a `sqlite3_value` SQLite owns for the duration of the
    // function callback that received it.
    let kind = unsafe { ffi::sqlite3_value_type(value) };
    match kind {
        ffi::SQLITE_INTEGER => {
            // SAFETY: same `value`; the integer accessor only reads it.
            let integer = unsafe { ffi::sqlite3_value_int64(value) };
            if big_int {
                return Some(v8::BigInt::new_from_i64(scope, integer).into());
            }
            if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&integer) {
                record_range_error(scope, state, integer);
                return None;
            }
            #[expect(
                clippy::cast_precision_loss,
                reason = "`integer` is within the JavaScript safe integer range, so the f64 is exact"
            )]
            let number = integer as f64;
            Some(v8::Number::new(scope, number).into())
        }
        ffi::SQLITE_FLOAT => {
            // SAFETY: same `value`; the double accessor only reads it.
            let float = unsafe { ffi::sqlite3_value_double(value) };
            Some(v8::Number::new(scope, float).into())
        }
        ffi::SQLITE_TEXT => {
            // SAFETY: same `value`; the returned pointer stays valid until SQLite
            // reclaims the value after this callback returns.
            let text = unsafe { ffi::sqlite3_value_text(value) };
            // SAFETY: same `value`; the length is the byte length of that text.
            let length = unsafe { ffi::sqlite3_value_bytes(value) };
            let bytes = slice(text.cast::<u8>(), length);
            Some(v8::String::new(scope, &String::from_utf8_lossy(bytes))?.into())
        }
        ffi::SQLITE_BLOB => {
            // SAFETY: same `value`; the returned pointer stays valid until SQLite
            // reclaims the value after this callback returns.
            let blob = unsafe { ffi::sqlite3_value_blob(value) };
            // SAFETY: same `value`; the length is the byte length of that blob.
            let length = unsafe { ffi::sqlite3_value_bytes(value) };
            Some(blob_view(scope, slice(blob.cast::<u8>(), length))?.into())
        }
        _ => Some(v8::null(scope).into()),
    }
}

pub(crate) fn to_sqlite(
    scope: &mut v8::PinScope<'_, '_>,
    context: *mut ffi::sqlite3_context,
    value: v8::Local<'_, v8::Value>,
) {
    if value.is_null_or_undefined() {
        // SAFETY: `context` is the function context SQLite passed to the callback
        // that produced this value.
        unsafe { ffi::sqlite3_result_null(context) };
    } else if value.is_number() {
        let number = value.number_value(scope).unwrap_or(f64::NAN);
        // SAFETY: same `context`.
        unsafe { ffi::sqlite3_result_double(context, number) };
    } else if value.is_big_int() {
        let (integer, lossless) = value
            .to_big_int(scope)
            .map_or((0, false), |integer| integer.i64_value());
        if lossless {
            // SAFETY: same `context`.
            unsafe { ffi::sqlite3_result_int64(context, integer) };
        } else {
            result_error(
                context,
                "BigInt value is too large to be returned from a function",
            );
        }
    } else if value.is_string() {
        let text = value.to_rust_string_lossy(scope);
        // SAFETY: `text` is a live Rust string until the end of this branch, and
        // SQLITE_TRANSIENT makes SQLite copy it before returning.
        unsafe {
            ffi::sqlite3_result_text64(
                context,
                text.as_ptr().cast::<c_char>(),
                text.len() as u64,
                ffi::SQLITE_TRANSIENT(),
                UTF8,
            );
        }
    } else if let Ok(view) = value.try_cast::<v8::ArrayBufferView>() {
        let mut bytes = vec![0; view.byte_length()];
        view.copy_contents(&mut bytes);
        result_blob(context, &bytes);
    } else if let Ok(buffer) = value.try_cast::<v8::ArrayBuffer>() {
        let length = buffer.byte_length();
        let bytes = buffer
            .data()
            .map(|data| {
                // SAFETY: `data` starts the buffer's backing store, which holds
                // `length` initialized bytes and stays alive with the buffer.
                unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), length) }
            })
            .unwrap_or_default();
        result_blob(context, bytes);
    } else {
        result_error(
            context,
            "Returned JavaScript value cannot be converted to a SQLite value",
        );
    }
}

fn record_range_error(
    scope: &mut v8::PinScope<'_, '_>,
    state: &Rc<RefCell<OpState>>,
    integer: i64,
) {
    let message = format!("Value is too large to be represented as a JavaScript number: {integer}");
    let Some(message) = v8::String::new(scope, &message) else {
        return;
    };
    let exception = v8::Exception::range_error(scope, message);
    let Some(object) = exception.to_object(scope) else {
        return;
    };
    if let Some(code) = v8::String::new(scope, OUT_OF_RANGE) {
        object.create_data_property(
            scope,
            v8::String::new(scope, "code").unwrap().into(),
            code.into(),
        );
    }
    record(scope, state, exception);
}

fn blob_view<'s>(
    scope: &v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> Option<v8::Local<'s, v8::Uint8Array>> {
    let buffer = v8::ArrayBuffer::new(scope, bytes.len());
    if !bytes.is_empty() {
        let data = buffer.data()?;
        // SAFETY: `buffer` was just allocated with `bytes.len()` bytes and no
        // other handle aliases its backing store yet.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.as_ptr().cast::<u8>(), bytes.len());
        }
    }
    v8::Uint8Array::new(scope, buffer, 0, bytes.len())
}

pub(crate) fn result_blob(context: *mut ffi::sqlite3_context, bytes: &[u8]) {
    // SAFETY: `bytes` is live for the duration of the call and SQLITE_TRANSIENT
    // makes SQLite copy it before returning.
    unsafe {
        ffi::sqlite3_result_blob64(
            context,
            bytes.as_ptr().cast(),
            bytes.len() as u64,
            ffi::SQLITE_TRANSIENT(),
        );
    }
}

pub(crate) fn result_error(context: *mut ffi::sqlite3_context, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: `message` is NUL-terminated and outlives the call; -1 makes SQLite
    // read up to the terminator.
    unsafe { ffi::sqlite3_result_error(context, message.as_ptr(), -1) };
}

pub(crate) const fn slice<'a>(pointer: *const u8, length: c_int) -> &'a [u8] {
    if pointer.is_null() || length <= 0 {
        &[]
    } else {
        #[expect(
            clippy::cast_sign_loss,
            reason = "the guard above proves `length` is positive"
        )]
        let length = length as usize;
        // SAFETY: the guard proves `pointer` is non-null and `length` positive;
        // callers only pass SQLite-owned buffers with that many valid bytes.
        unsafe { std::slice::from_raw_parts(pointer, length) }
    }
}
