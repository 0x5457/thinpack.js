use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed(
            "node:string_decoder",
            thinpack_js::js!("node-string-decoder.js"),
        ),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/binding/string_decoder",
            thinpack_js::js!("binding_string_decoder.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_string_decoder",
        deps: &["thinpack_node_buffer"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
