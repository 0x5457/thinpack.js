use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};
use thinpack_core::IsolateInputs;

/// Builds the `node:ffi` bindings and the Deno FFI engine they call.
#[must_use]
pub fn init(_isolate: &IsolateInputs) -> Vec<Extension> {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:ffi",
        thinpack_js::js!("ffi.js"),
    )];

    let bindings = Extension {
        name: "thinpack_node_ffi",
        deps: &["thinpack_node_internals"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    };
    vec![bindings, deno_ffi::init_embedded()]
}
