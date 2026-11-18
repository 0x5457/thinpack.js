use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:v8",
        thinpack_js::js!("v8.js"),
    )];

    Extension {
        name: "thinpack_node_v8",
        deps: &["thinpack_node_internals", "thinpack_node_buffer"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
