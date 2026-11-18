use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new_compressed("node:http", thinpack_js::js!("http.js")),
        ExtensionFileSource::new_compressed(
            "node:thinpack/internal/websocket",
            thinpack_js::js!("websocket.js"),
        ),
    ];

    Extension {
        name: "thinpack_node_http",
        deps: &[
            "thinpack_node_internals",
            "thinpack_node_stream",
            "thinpack_node_net",
        ],
        lazy_loaded_js_files: Cow::Borrowed(BUILTINS),
        ..Default::default()
    }
}
