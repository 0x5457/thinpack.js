use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new(
            "node:readline",
            deno_core::ascii_str_include!("readline.js"),
        ),
        ExtensionFileSource::new(
            "node:readline/promises",
            deno_core::ascii_str_include!("readline_promises.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_readline",
        deps: &["thinpack_node_internals", "thinpack_node_events"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
