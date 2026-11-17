use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:ffi",
        deno_core::ascii_str_include!("ffi.js"),
    )];

    Extension {
        name: "thinpack_node_ffi",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}

/// The vendored FFI engine. It is a separate extension from the `node:ffi`
/// bindings, so the runtime registers both when the capability is enabled.
#[must_use]
pub fn engine() -> Extension {
    deno_ffi::init_embedded()
}
