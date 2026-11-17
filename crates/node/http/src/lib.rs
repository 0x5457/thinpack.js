use std::borrow::Cow;

use deno_core::{Extension, ExtensionFileSource};

#[must_use]
pub fn init() -> Extension {
    const BUILTINS: &[ExtensionFileSource] = &[
        ExtensionFileSource::new("node:http", deno_core::ascii_str_include!("http.js")),
        ExtensionFileSource::new(
            "node:thinpack/internal/websocket",
            deno_core::ascii_str_include!("websocket.js"),
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
