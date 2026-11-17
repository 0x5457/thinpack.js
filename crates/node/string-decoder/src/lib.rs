use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:string_decoder",
            deno_core::ascii_str_include!("node-string-decoder.js"),
        ),
        ExtensionFileSource::new(
            "node:thinpack/internal/binding/string_decoder",
            deno_core::ascii_str_include!("binding_string_decoder.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_string_decoder",
        deps: &["thinpack_node_buffer"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
