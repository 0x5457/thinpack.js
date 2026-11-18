use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:readline", thinpack_js::js!("readline.js")),
        ExtensionFileSource::new_compressed(
            "node:readline/promises",
            thinpack_js::js!("readline_promises.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_readline",
        deps: &["thinpack_node_internals", "thinpack_node_events"],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
