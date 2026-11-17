use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new(
        "node:v8",
        deno_core::ascii_str_include!("v8.js"),
    )];

    Extension {
        name: "thinpack_node_v8",
        deps: &["thinpack_node_internals", "thinpack_node_buffer"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
