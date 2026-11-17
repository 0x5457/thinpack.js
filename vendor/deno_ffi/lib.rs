#![allow(deprecated)]
// Copyright 2018-2026 the Deno authors. MIT license.

use std::mem::size_of;
use std::os::raw::c_char;
use std::os::raw::c_short;

mod call;
mod callback;
mod dlfcn;
mod ir;
mod permissions;
mod repr;
mod r#static;
mod symbol;
mod turbocall;

pub use call::CallError;
use call::op_ffi_call_nonblocking;
use call::op_ffi_call_ptr;
use call::op_ffi_call_ptr_nonblocking;
pub use callback::CallbackError;
use callback::op_ffi_unsafe_callback_close;
use callback::op_ffi_unsafe_callback_create;
use callback::op_ffi_unsafe_callback_ref;
pub use dlfcn::DlfcnError;
use dlfcn::ForeignFunction;
use dlfcn::op_ffi_load;
pub use ir::IRError;
pub use repr::ReprError;
use repr::*;
pub use r#static::StaticError;
use r#static::op_ffi_get_static;
use symbol::NativeType;
use symbol::Symbol;
use turbocall::op_ffi_get_turbocall_target;

#[cfg(not(target_pointer_width = "64"))]
compile_error!("platform not supported");

const _: () = {
    assert!(size_of::<c_char>() == 1);
    assert!(size_of::<c_short>() == 2);
    assert!(size_of::<*const ()>() == 8);
};

pub const UNSTABLE_FEATURE_NAME: &str = "ffi";

deno_core::extension!(deno_ffi,
  deps = [],
  ops = [
    op_ffi_load,
    op_ffi_get_static,
    op_ffi_call_nonblocking,
    op_ffi_call_ptr,
    op_ffi_call_ptr_nonblocking,
    op_ffi_ptr_create,
    op_ffi_ptr_equals,
    op_ffi_ptr_of,
    op_ffi_ptr_of_exact,
    op_ffi_ptr_offset,
    op_ffi_ptr_value,
    op_ffi_get_buf,
    op_ffi_buf_copy_into,
    op_ffi_cstr_read,
    op_ffi_read_bool,
    op_ffi_read_u8,
    op_ffi_read_i8,
    op_ffi_read_u16,
    op_ffi_read_i16,
    op_ffi_read_u32,
    op_ffi_read_i32,
    op_ffi_read_u64,
    op_ffi_read_i64,
    op_ffi_read_f32,
    op_ffi_read_f64,
    op_ffi_read_ptr,
    op_ffi_unsafe_callback_create,
    op_ffi_unsafe_callback_close,
    op_ffi_unsafe_callback_ref,
    op_ffi_get_turbocall_target,
  ],
  lazy_loaded_js = [ "00_ffi.js" with_specifier "ext:deno_ffi/00_ffi.js" ],
  state = |state| {
    state.put(permissions::PermissionsContainer);
  },
);

/// The extension's generated `lazy_loaded_js` points at a file on disk, which
/// thinpack does not allow. Build the same extension with the script embedded so it
/// works in a packed single-file runtime too.
pub fn init_embedded() -> deno_core::Extension {
    let mut extension = deno_ffi::init();
    extension.lazy_loaded_js_files =
        std::borrow::Cow::Owned(vec![deno_core::ExtensionFileSource::new(
            "ext:deno_ffi/00_ffi.js",
            deno_core::ascii_str_include!("00_ffi.js"),
        )]);
    extension
}
