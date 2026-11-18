use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[ExtensionFileSource::new_compressed(
        "node:https",
        thinpack_js::js!("https.js"),
    )];

    Extension {
        name: "thinpack_node_https",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_http",
            "thinpack_node_tls",
        ],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
